// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Allocation-free inspection of the LiteRT-LM container envelope and section
//! table.
//!
//! This module intentionally stops at the package boundary.  A valid container
//! is not evidence that InterOS can execute its TFLite graphs.  Execution also
//! requires native Rust graph, tokenizer, operator and quantization support.

pub const MAGIC: &[u8; 8] = b"LITERTLM";
pub const TFLITE_FILE_IDENTIFIER: &[u8; 4] = b"TFL3";
pub const ENVELOPE_BYTES: usize = 32;
pub const HEADER_BEGIN: usize = 32;
pub const BLOCK_SIZE: u64 = 16 * 1024;
pub const SUPPORTED_MAJOR: u32 = 1;
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_SECTIONS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub version: Version,
    pub header_end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SectionType {
    None = 0,
    GenericBinaryData = 1,
    Deprecated = 2,
    TfLiteModel = 3,
    SentencePieceTokenizer = 4,
    LlmMetadataProto = 5,
    HuggingFaceTokenizerZlib = 6,
    TfLiteWeights = 7,
    EmbeddingMetadataProto = 8,
    ExecutorMetadataProto = 9,
    TextToSpeechMetadataProto = 10,
    SpeechToTextMetadataProto = 11,
    TextToImageMetadataProto = 12,
    AnyToAnyMetadataProto = 13,
}

impl SectionType {
    fn from_byte(value: u8) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::GenericBinaryData),
            2 => Ok(Self::Deprecated),
            3 => Ok(Self::TfLiteModel),
            4 => Ok(Self::SentencePieceTokenizer),
            5 => Ok(Self::LlmMetadataProto),
            6 => Ok(Self::HuggingFaceTokenizerZlib),
            7 => Ok(Self::TfLiteWeights),
            8 => Ok(Self::EmbeddingMetadataProto),
            9 => Ok(Self::ExecutorMetadataProto),
            10 => Ok(Self::TextToSpeechMetadataProto),
            11 => Ok(Self::SpeechToTextMetadataProto),
            12 => Ok(Self::TextToImageMetadataProto),
            13 => Ok(Self::AnyToAnyMetadataProto),
            _ => Err(Error::UnknownSectionType),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub data_type: SectionType,
    pub begin: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub envelope: Envelope,
    pub section_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TfLiteTensorMeta {
    pub tensor_type: u8,
    pub buffer_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TfLiteSubgraphMeta {
    pub tensor_count: usize,
    pub operator_count: usize,
    pub input_count: usize,
    pub output_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TfLiteModelInspection {
    pub version: u32,
    pub operator_codes_count: usize,
    pub subgraphs_count: usize,
    pub buffers_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    BadMagic,
    UnsupportedMajor,
    HeaderBeforeMetadata,
    HeaderTooLarge,
    HeaderPastFile,
    InvalidFlatBuffer,
    MissingSectionMetadata,
    TooManySections,
    UnknownSectionType,
    SectionBeforePayload,
    SectionUnaligned,
    SectionRange,
    SectionOverlap,
    BadTfLiteIdentifier,
}

pub fn parse_envelope(prefix: &[u8], file_size: u64) -> Result<Envelope, Error> {
    if prefix.len() < ENVELOPE_BYTES {
        return Err(Error::Truncated);
    }
    if &prefix[..MAGIC.len()] != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = Version {
        major: read_u32(prefix, 8)?,
        minor: read_u32(prefix, 12)?,
        patch: read_u32(prefix, 16)?,
    };
    if version.major != SUPPORTED_MAJOR {
        return Err(Error::UnsupportedMajor);
    }
    let header_end_u64 = read_u64(prefix, 24)?;
    let header_end = usize::try_from(header_end_u64).map_err(|_| Error::HeaderTooLarge)?;
    if header_end <= HEADER_BEGIN {
        return Err(Error::HeaderBeforeMetadata);
    }
    if header_end > MAX_HEADER_BYTES {
        return Err(Error::HeaderTooLarge);
    }
    if header_end_u64 > file_size {
        return Err(Error::HeaderPastFile);
    }
    Ok(Envelope {
        version,
        header_end,
    })
}

/// Enumerate validated sections from a package header.
pub fn parse_sections<F>(header: &[u8], file_size: u64, mut callback: F) -> Result<Envelope, Error>
where
    F: FnMut(usize, Section) -> Result<(), Error>,
{
    let envelope = parse_envelope(header, file_size)?;
    if header.len() < envelope.header_end {
        return Err(Error::Truncated);
    }
    let metadata = &header[HEADER_BEGIN..envelope.header_end];
    let reader = FlatBuffer::new(metadata);
    let root = reader.follow_uoffset(0)?;
    // LiteRTLMMetaData.section_metadata is field 1.
    let section_metadata_field = reader
        .table_field(root, 1, 4)?
        .ok_or(Error::MissingSectionMetadata)?;
    let section_metadata = reader.follow_uoffset(section_metadata_field)?;
    // SectionMetadata.objects is field 0.
    let objects_field = reader
        .table_field(section_metadata, 0, 4)?
        .ok_or(Error::MissingSectionMetadata)?;
    let objects = reader.follow_uoffset(objects_field)?;
    let section_count = reader.u32(objects)? as usize;
    if section_count > MAX_SECTIONS {
        return Err(Error::TooManySections);
    }

    let payload_begin =
        round_up(envelope.header_end as u64, BLOCK_SIZE).ok_or(Error::SectionRange)?;
    let mut previous_end = payload_begin;
    for index in 0..section_count {
        let slot = objects
            .checked_add(4)
            .and_then(|value| value.checked_add(index.checked_mul(4)?))
            .ok_or(Error::InvalidFlatBuffer)?;
        let object = reader.follow_uoffset(slot)?;
        let section = read_section(&reader, object)?;
        if section.begin < payload_begin {
            return Err(Error::SectionBeforePayload);
        }
        if section.begin % BLOCK_SIZE != 0 {
            return Err(Error::SectionUnaligned);
        }
        if section.end < section.begin || section.end > file_size {
            return Err(Error::SectionRange);
        }
        let required_begin = round_up(previous_end, BLOCK_SIZE).ok_or(Error::SectionRange)?;
        if section.begin < required_begin {
            return Err(Error::SectionOverlap);
        }
        previous_end = section.end;
        callback(index, section)?;
    }

    Ok(envelope)
}

/// Validates the package envelope, FlatBuffer section table and every payload
/// range visible in the supplied header.
///
/// `header` must contain bytes `[0, envelope.header_end)`.  Callers can read
/// just that bounded prefix; model payloads are never copied into memory.
pub fn inspect(header: &[u8], file_size: u64) -> Result<Inspection, Error> {
    let mut section_count = 0;
    let envelope = parse_sections(header, file_size, |_, _| {
        section_count += 1;
        Ok(())
    })?;
    Ok(Inspection {
        envelope,
        section_count,
    })
}

fn read_section(reader: &FlatBuffer<'_>, object: usize) -> Result<Section, Error> {
    // SectionObject: items=0, begin_offset=1, end_offset=2, data_type=3.
    let begin = reader
        .table_field(object, 1, 8)?
        .ok_or(Error::InvalidFlatBuffer)
        .and_then(|offset| reader.u64(offset))?;
    let end = reader
        .table_field(object, 2, 8)?
        .ok_or(Error::InvalidFlatBuffer)
        .and_then(|offset| reader.u64(offset))?;
    let data_type = match reader.table_field(object, 3, 1)? {
        Some(offset) => SectionType::from_byte(reader.byte(offset)?)?,
        None => SectionType::None,
    };
    Ok(Section {
        data_type,
        begin,
        end,
    })
}

fn round_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment.checked_sub(1)?)
        .map(|result| result / alignment * alignment)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    let raw: [u8; 4] = bytes
        .get(offset..offset.checked_add(4).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?
        .try_into()
        .map_err(|_| Error::Truncated)?;
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    let raw: [u8; 8] = bytes
        .get(offset..offset.checked_add(8).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?
        .try_into()
        .map_err(|_| Error::Truncated)?;
    Ok(u64::from_le_bytes(raw))
}

struct FlatBuffer<'a> {
    bytes: &'a [u8],
}

impl<'a> FlatBuffer<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn byte(&self, offset: usize) -> Result<u8, Error> {
        self.bytes
            .get(offset)
            .copied()
            .ok_or(Error::InvalidFlatBuffer)
    }

    fn u16(&self, offset: usize) -> Result<u16, Error> {
        let raw: [u8; 2] = self
            .bytes
            .get(offset..offset.checked_add(2).ok_or(Error::InvalidFlatBuffer)?)
            .ok_or(Error::InvalidFlatBuffer)?
            .try_into()
            .map_err(|_| Error::InvalidFlatBuffer)?;
        Ok(u16::from_le_bytes(raw))
    }

    fn u32(&self, offset: usize) -> Result<u32, Error> {
        read_u32(self.bytes, offset).map_err(|_| Error::InvalidFlatBuffer)
    }

    fn i32(&self, offset: usize) -> Result<i32, Error> {
        Ok(self.u32(offset)? as i32)
    }

    fn u64(&self, offset: usize) -> Result<u64, Error> {
        read_u64(self.bytes, offset).map_err(|_| Error::InvalidFlatBuffer)
    }

    fn follow_uoffset(&self, offset: usize) -> Result<usize, Error> {
        let distance = self.u32(offset)? as usize;
        let target = offset
            .checked_add(distance)
            .ok_or(Error::InvalidFlatBuffer)?;
        if target >= self.bytes.len() {
            return Err(Error::InvalidFlatBuffer);
        }
        Ok(target)
    }

    fn table_field(
        &self,
        table: usize,
        field: usize,
        width: usize,
    ) -> Result<Option<usize>, Error> {
        let backwards = self.i32(table)?;
        if backwards == 0 {
            return Err(Error::InvalidFlatBuffer);
        }
        // FlatBuffers normally place a vtable before its table.  Vtable
        // deduplication may reuse one after the table, represented by a
        // negative signed offset (both pinned Google packages do this).
        let distance = backwards.unsigned_abs() as usize;
        let vtable = if backwards > 0 {
            table.checked_sub(distance)
        } else {
            table.checked_add(distance)
        }
        .ok_or(Error::InvalidFlatBuffer)?;
        let vtable_bytes = self.u16(vtable)? as usize;
        let object_bytes =
            self.u16(vtable.checked_add(2).ok_or(Error::InvalidFlatBuffer)?)? as usize;
        if vtable_bytes < 4
            || vtable_bytes % 2 != 0
            || object_bytes < 4
            || vtable
                .checked_add(vtable_bytes)
                .filter(|end| *end <= self.bytes.len())
                .is_none()
            || table
                .checked_add(object_bytes)
                .filter(|end| *end <= self.bytes.len())
                .is_none()
        {
            return Err(Error::InvalidFlatBuffer);
        }
        let entry = 4usize
            .checked_add(field.checked_mul(2).ok_or(Error::InvalidFlatBuffer)?)
            .ok_or(Error::InvalidFlatBuffer)?;
        if entry.checked_add(2).ok_or(Error::InvalidFlatBuffer)? > vtable_bytes {
            return Ok(None);
        }
        let field_offset =
            self.u16(vtable.checked_add(entry).ok_or(Error::InvalidFlatBuffer)?)? as usize;
        if field_offset == 0 {
            return Ok(None);
        }
        if field_offset < 4
            || field_offset
                .checked_add(width)
                .filter(|end| *end <= object_bytes)
                .is_none()
        {
            return Err(Error::InvalidFlatBuffer);
        }
        let absolute = table
            .checked_add(field_offset)
            .ok_or(Error::InvalidFlatBuffer)?;
        if absolute >= self.bytes.len() {
            return Err(Error::InvalidFlatBuffer);
        }
        Ok(Some(absolute))
    }

    fn vector_len(&self, vector_offset: usize) -> Result<usize, Error> {
        let len = self.u32(vector_offset)?;
        Ok(len as usize)
    }

    fn vector_i32_slice(&self, vector_offset: usize, out: &mut [i32]) -> Result<usize, Error> {
        let len = self.vector_len(vector_offset)?;
        let copy_count = len.min(out.len());
        for i in 0..copy_count {
            let elem_offset = vector_offset
                .checked_add(4)
                .and_then(|val| val.checked_add(i.checked_mul(4)?))
                .ok_or(Error::InvalidFlatBuffer)?;
            out[i] = self.i32(elem_offset)?;
        }
        Ok(len)
    }

    fn vector_u8_slice(&self, vector_offset: usize) -> Result<&'a [u8], Error> {
        let len = self.vector_len(vector_offset)?;
        let start = vector_offset.checked_add(4).ok_or(Error::InvalidFlatBuffer)?;
        let end = start.checked_add(len).ok_or(Error::InvalidFlatBuffer)?;
        if end > self.bytes.len() {
            return Err(Error::InvalidFlatBuffer);
        }
        Ok(&self.bytes[start..end])
    }

    fn vector_element_offset(
        &self,
        vector_offset: usize,
        index: usize,
        element_size: usize,
    ) -> Result<usize, Error> {
        let count = self.vector_len(vector_offset)?;
        if index >= count {
            return Err(Error::InvalidFlatBuffer);
        }
        let elem_offset = vector_offset
            .checked_add(4)
            .and_then(|val| val.checked_add(index.checked_mul(element_size)?))
            .ok_or(Error::InvalidFlatBuffer)?;
        if elem_offset
            .checked_add(element_size)
            .filter(|end| *end <= self.bytes.len())
            .is_none()
        {
            return Err(Error::InvalidFlatBuffer);
        }
        Ok(elem_offset)
    }

    fn vector_table(&self, vector_offset: usize, index: usize) -> Result<usize, Error> {
        let slot = self.vector_element_offset(vector_offset, index, 4)?;
        self.follow_uoffset(slot)
    }

    fn table_vector(&self, table: usize, field: usize) -> Result<Option<usize>, Error> {
        match self.table_field(table, field, 4)? {
            Some(field_offset) => Ok(Some(self.follow_uoffset(field_offset)?)),
            None => Ok(None),
        }
    }
}

/// Inspects the root `Model` table of a TFLite FlatBuffer.
pub fn inspect_tflite_model(bytes: &[u8]) -> Result<TfLiteModelInspection, Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;

    let version = match reader.table_field(root, 0, 4)? {
        Some(offset) => reader.u32(offset)?,
        None => 0,
    };
    let operator_codes_count = match reader.table_vector(root, 1)? {
        Some(vec_off) => reader.vector_len(vec_off)?,
        None => 0,
    };
    let subgraphs_count = match reader.table_vector(root, 2)? {
        Some(vec_off) => reader.vector_len(vec_off)?,
        None => 0,
    };
    let buffers_count = match reader.table_vector(root, 4)? {
        Some(vec_off) => reader.vector_len(vec_off)?,
        None => 0,
    };
    Ok(TfLiteModelInspection {
        version,
        operator_codes_count,
        subgraphs_count,
        buffers_count,
    })
}

/// Inspects a specific subgraph in a TFLite FlatBuffer.
pub fn inspect_tflite_subgraph(
    bytes: &[u8],
    subgraph_index: usize,
) -> Result<TfLiteSubgraphMeta, Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;
    let subgraphs_vec = reader
        .table_vector(root, 2)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let sg = reader.vector_table(subgraphs_vec, subgraph_index)?;

    let tensor_count = match reader.table_vector(sg, 0)? {
        Some(vec_off) => reader.vector_len(vec_off)?,
        None => 0,
    };
    let input_count = match reader.table_vector(sg, 1)? {
        Some(vec_off) => reader.vector_len(vec_off)?,
        None => 0,
    };
    let output_count = match reader.table_vector(sg, 2)? {
        Some(vec_off) => reader.vector_len(vec_off)?,
        None => 0,
    };
    let operator_count = match reader.table_vector(sg, 3)? {
        Some(vec_off) => reader.vector_len(vec_off)?,
        None => 0,
    };

    Ok(TfLiteSubgraphMeta {
        tensor_count,
        operator_count,
        input_count,
        output_count,
    })
}

/// Inspects metadata for a specific tensor within a subgraph.
pub fn inspect_tflite_tensor(
    bytes: &[u8],
    subgraph_index: usize,
    tensor_index: usize,
) -> Result<TfLiteTensorMeta, Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;
    let subgraphs_vec = reader
        .table_vector(root, 2)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let sg = reader.vector_table(subgraphs_vec, subgraph_index)?;
    let tensors_vec = reader
        .table_vector(sg, 0)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let tensor = reader.vector_table(tensors_vec, tensor_index)?;

    let tensor_type = match reader.table_field(tensor, 1, 1)? {
        Some(offset) => reader.byte(offset)?,
        None => 0,
    };
    let buffer_index = match reader.table_field(tensor, 2, 4)? {
        Some(offset) => reader.u32(offset)?,
        None => 0,
    };

    Ok(TfLiteTensorMeta {
        tensor_type,
        buffer_index,
    })
}

/// Inspects the operator code index for an operator in a subgraph.
pub fn inspect_tflite_operator_opcode_index(
    bytes: &[u8],
    subgraph_index: usize,
    operator_index: usize,
) -> Result<u32, Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;
    let subgraphs_vec = reader
        .table_vector(root, 2)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let sg = reader.vector_table(subgraphs_vec, subgraph_index)?;
    let operators_vec = reader
        .table_vector(sg, 3)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let op = reader.vector_table(operators_vec, operator_index)?;

    match reader.table_field(op, 0, 4)? {
        Some(offset) => reader.u32(offset),
        None => Ok(0),
    }
}

/// Inspects the shape of a specific tensor within a subgraph.
/// Returns the actual rank of the tensor, writing up to `out_shape.len()` dimensions.
pub fn inspect_tflite_tensor_shape(
    bytes: &[u8],
    subgraph_index: usize,
    tensor_index: usize,
    out_shape: &mut [i32],
) -> Result<usize, Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;
    let subgraphs_vec = reader
        .table_vector(root, 2)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let sg = reader.vector_table(subgraphs_vec, subgraph_index)?;
    let tensors_vec = reader
        .table_vector(sg, 0)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let tensor = reader.vector_table(tensors_vec, tensor_index)?;

    match reader.table_vector(tensor, 0)? {
        Some(vec_off) => reader.vector_i32_slice(vec_off, out_shape),
        None => Ok(0),
    }
}

/// Inspects input and output tensor indices for a specific operator.
pub fn inspect_tflite_operator_io(
    bytes: &[u8],
    subgraph_index: usize,
    operator_index: usize,
    in_indices: &mut [i32],
    out_indices: &mut [i32],
) -> Result<(usize, usize), Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;
    let subgraphs_vec = reader
        .table_vector(root, 2)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let sg = reader.vector_table(subgraphs_vec, subgraph_index)?;
    let operators_vec = reader
        .table_vector(sg, 3)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let op = reader.vector_table(operators_vec, operator_index)?;

    let in_count = match reader.table_vector(op, 1)? {
        Some(vec_off) => reader.vector_i32_slice(vec_off, in_indices)?,
        None => 0,
    };
    let out_count = match reader.table_vector(op, 2)? {
        Some(vec_off) => reader.vector_i32_slice(vec_off, out_indices)?,
        None => 0,
    };

    Ok((in_count, out_count))
}

/// Inspects a builtin operator code from the Model's operator_codes list.
pub fn inspect_tflite_operator_code(
    bytes: &[u8],
    opcode_index: usize,
) -> Result<i32, Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;
    let opcodes_vec = reader
        .table_vector(root, 1)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let op_code_table = reader.vector_table(opcodes_vec, opcode_index)?;

    // Field 3: builtin_code (int32). If not present or 0, check field 0: deprecated_builtin_code (byte).
    if let Some(offset) = reader.table_field(op_code_table, 3, 4)? {
        let code = reader.i32(offset)?;
        if code != 0 {
            return Ok(code);
        }
    }
    if let Some(offset) = reader.table_field(op_code_table, 0, 1)? {
        let code = reader.byte(offset)? as i32;
        return Ok(code);
    }
    Ok(0)
}

/// Inspects a constant data buffer from the Model's buffers list.
pub fn inspect_tflite_buffer<'a>(
    bytes: &'a [u8],
    buffer_index: usize,
) -> Result<Option<&'a [u8]>, Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;
    let buffers_vec = reader
        .table_vector(root, 4)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let buf_table = reader.vector_table(buffers_vec, buffer_index)?;

    match reader.table_vector(buf_table, 0)? {
        Some(vec_off) => Ok(Some(reader.vector_u8_slice(vec_off)?)),
        None => Ok(None),
    }
}

/// Extracts the byte offset and count of float32 scales for a quantized tensor in a subgraph.
pub fn extract_tflite_tensor_quantization_scales_offset(
    bytes: &[u8],
    subgraph_index: usize,
    tensor_index: usize,
) -> Result<(usize, usize), Error> {
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    if &bytes[4..8] != TFLITE_FILE_IDENTIFIER {
        return Err(Error::BadTfLiteIdentifier);
    }
    let reader = FlatBuffer::new(bytes);
    let root = reader.follow_uoffset(0)?;
    let subgraphs_vec = reader
        .table_vector(root, 2)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let sg = reader.vector_table(subgraphs_vec, subgraph_index)?;
    let tensors_vec = reader
        .table_vector(sg, 0)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let tensor = reader.vector_table(tensors_vec, tensor_index)?;

    // Field 4: quantization (QuantizationParameters)
    let q_field = reader
        .table_field(tensor, 4, 4)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let q_tbl = reader.follow_uoffset(q_field)?;

    // QuantizationParameters field 2: scale ([float])
    let scale_field = reader
        .table_field(q_tbl, 2, 4)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let scale_vec = reader.follow_uoffset(scale_field)?;
    let num_scales = reader.vector_len(scale_vec)?;

    let offset = scale_vec
        .checked_add(4)
        .ok_or(Error::InvalidFlatBuffer)?;
    let total_bytes = num_scales
        .checked_mul(4)
        .ok_or(Error::InvalidFlatBuffer)?;
    if offset.checked_add(total_bytes).filter(|end| *end <= bytes.len()).is_none() {
        return Err(Error::InvalidFlatBuffer);
    }

    Ok((offset, num_scales))
}

/// Kernel: Element-wise or broadcast `a >= b`.
pub fn kernel_greater_equal(a: &[i32], b: &[i32], out: &mut [bool]) -> Result<(), Error> {
    if out.len() < a.len() {
        return Err(Error::Truncated);
    }
    if b.len() == 1 {
        let val = b[0];
        for (i, &x) in a.iter().enumerate() {
            out[i] = x >= val;
        }
    } else if a.len() == b.len() {
        for i in 0..a.len() {
            out[i] = a[i] >= b[i];
        }
    } else {
        return Err(Error::InvalidFlatBuffer);
    }
    Ok(())
}

/// Kernel: Element-wise or broadcast `a < b`.
pub fn kernel_less(a: &[i32], b: &[i32], out: &mut [bool]) -> Result<(), Error> {
    if out.len() < a.len() {
        return Err(Error::Truncated);
    }
    if b.len() == 1 {
        let val = b[0];
        for (i, &x) in a.iter().enumerate() {
            out[i] = x < val;
        }
    } else if a.len() == b.len() {
        for i in 0..a.len() {
            out[i] = a[i] < b[i];
        }
    } else {
        return Err(Error::InvalidFlatBuffer);
    }
    Ok(())
}

/// Kernel: Element-wise logical AND `a && b`.
pub fn kernel_logical_and(a: &[bool], b: &[bool], out: &mut [bool]) -> Result<(), Error> {
    if a.len() != b.len() || out.len() < a.len() {
        return Err(Error::Truncated);
    }
    for i in 0..a.len() {
        out[i] = a[i] && b[i];
    }
    Ok(())
}

/// Kernel: Select V2 (`cond ? true_val : false_val`).
pub fn kernel_select_v2<T: Copy>(
    cond: &[bool],
    true_val: &[T],
    false_val: &[T],
    out: &mut [T],
) -> Result<(), Error> {
    if cond.len() != true_val.len() || out.len() < cond.len() {
        return Err(Error::Truncated);
    }
    if false_val.len() == 1 {
        let fallback = false_val[0];
        for i in 0..cond.len() {
            out[i] = if cond[i] { true_val[i] } else { fallback };
        }
    } else if false_val.len() == true_val.len() {
        for i in 0..cond.len() {
            out[i] = if cond[i] { true_val[i] } else { false_val[i] };
        }
    } else {
        return Err(Error::InvalidFlatBuffer);
    }
    Ok(())
}

/// Kernel: Reshape (contiguous tensor element transfer).
pub fn kernel_reshape<T: Copy>(input: &[T], out: &mut [T]) -> Result<(), Error> {
    if input.len() != out.len() {
        return Err(Error::Truncated);
    }
    out.copy_from_slice(input);
    Ok(())
}

/// Kernel: Gather bfloat16 weights converted to float32.
pub fn kernel_gather_bfloat16_to_f32(
    indices: &[i32],
    weights: &[u16],
    dim: usize,
    out: &mut [f32],
) -> Result<(), Error> {
    if out.len() < indices.len() * dim {
        return Err(Error::Truncated);
    }
    for (i, &idx) in indices.iter().enumerate() {
        let idx_usize = usize::try_from(idx).map_err(|_| Error::SectionRange)?;
        let start = idx_usize.checked_mul(dim).ok_or(Error::SectionRange)?;
        let end = start.checked_add(dim).ok_or(Error::SectionRange)?;
        if end > weights.len() {
            return Err(Error::SectionRange);
        }
        let out_start = i * dim;
        for (d, &bf) in weights[start..end].iter().enumerate() {
            out[out_start + d] = f32::from_bits((bf as u32) << 16);
        }
    }
    Ok(())
}

/// Kernel: In-place scalar multiplication on float32 slices.
pub fn kernel_mul_scalar_f32(data: &mut [f32], scalar: f32) {
    for x in data.iter_mut() {
        *x *= scalar;
    }
}

/// Kernel: Gather INT4 weights with per-token scale factors.
///
/// Unpacks two 4-bit signed 2's complement nibbles (-8..=7) per byte,
/// multiplies each element by the token's scale factor, and writes to `out`.
pub fn kernel_gather_int4_scaled(
    indices: &[i32],
    weights: &[u8],
    scales: &[f32],
    dim: usize,
    out: &mut [f32],
) -> Result<(), Error> {
    if dim % 2 != 0 {
        return Err(Error::InvalidFlatBuffer);
    }
    let bytes_per_token = dim / 2;
    if out.len() < indices.len() * dim {
        return Err(Error::Truncated);
    }
    for (i, &idx) in indices.iter().enumerate() {
        let idx_usize = usize::try_from(idx).map_err(|_| Error::SectionRange)?;
        if idx_usize >= scales.len() {
            return Err(Error::SectionRange);
        }
        let scale = scales[idx_usize];
        let start = idx_usize.checked_mul(bytes_per_token).ok_or(Error::SectionRange)?;
        let end = start.checked_add(bytes_per_token).ok_or(Error::SectionRange)?;
        if end > weights.len() {
            return Err(Error::SectionRange);
        }
        let out_start = i * dim;
        for (b_idx, &byte) in weights[start..end].iter().enumerate() {
            let lo = byte & 0x0f;
            let hi = (byte >> 4) & 0x0f;
            let lo_signed = if lo < 8 { lo as f32 } else { (lo as i8 - 16) as f32 };
            let hi_signed = if hi < 8 { hi as f32 } else { (hi as i8 - 16) as f32 };
            out[out_start + b_idx * 2] = lo_signed * scale;
            out[out_start + b_idx * 2 + 1] = hi_signed * scale;
        }
    }
    Ok(())
}

/// Kernel: RMS Normalization matching `odml.rms_norm.impl`:
/// y_i = (x_i / sqrt(mean(x^2) + eps)) * w_i
pub fn kernel_rms_norm(x: &mut [f32], w: &[f32], eps: f32) -> Result<(), Error> {
    if x.is_empty() || w.len() < x.len() || eps <= 0.0 {
        return Err(Error::Truncated);
    }
    let dim = x.len();
    let mut sum_sq = 0.0f32;
    for &v in x.iter() {
        sum_sq += v * v;
    }
    let mean_sq = sum_sq / (dim as f32);
    let scale = 1.0f32 / crate::f32_sqrt(mean_sq + eps);
    for i in 0..dim {
        x[i] = x[i] * scale * w[i];
    }
    Ok(())
}

/// Kernel: GELU with approximate tanh matching TFLite GELU (`approximate=true`):
/// GELU(x) = 0.5 * x * (1 + tanh(sqrt(2/pi) * (x + 0.044715 * x^3)))
pub fn kernel_gelu(x: &mut [f32]) {
    for val in x.iter_mut() {
        *val = crate::inference::gelu_tanh(*val);
    }
}

/// Kernel: Numerically stable in-place Softmax over a vector:
/// softmax(x)_i = exp(x_i - max(x)) / sum(exp(x_j - max(x)))
pub fn kernel_softmax(x: &mut [f32]) -> Result<(), Error> {
    if x.is_empty() {
        return Err(Error::Truncated);
    }
    let mut max_val = x[0];
    for &v in &x[1..] {
        if v > max_val {
            max_val = v;
        }
    }
    let mut sum = 0.0f32;
    for v in x.iter_mut() {
        *v = crate::inference::f32_exp(*v - max_val);
        sum += *v;
    }
    if sum > 0.0 {
        let inv_sum = 1.0f32 / sum;
        for v in x.iter_mut() {
            *v *= inv_sum;
        }
    }
    Ok(())
}

/// Kernel: Matrix multiplication out = A * B^T
/// A is [M, K], B is [N, K] (so B^T is [K, N]), out is [M, N]
pub fn kernel_matmul_f32(
    a: &[f32],
    b: &[f32],
    m: usize,
    k: usize,
    n: usize,
    out: &mut [f32],
) -> Result<(), Error> {
    if a.len() < m * k || b.len() < n * k || out.len() < m * n {
        return Err(Error::Truncated);
    }
    for row in 0..m {
        let a_row = &a[row * k..(row + 1) * k];
        for col in 0..n {
            let b_row = &b[col * k..(col + 1) * k];
            let mut dot = 0.0f32;
            for i in 0..k {
                dot += a_row[i] * b_row[i];
            }
            out[row * n + col] = dot;
        }
    }
    Ok(())
}

/// Kernel: INT4 quantized matrix multiplication out = A * W^T
/// A is [M, K] in float32.
/// W is [N, K/2] packed INT4 bytes (signed 4-bit nibbles -8..=7, 2 per byte).
/// scales is [N] float32 scale factors (one per output channel/row of W).
/// out is [M, N] in float32.
pub fn kernel_matmul_int4_scaled(
    a: &[f32],
    w: &[u8],
    scales: &[f32],
    m: usize,
    k: usize,
    n: usize,
    out: &mut [f32],
) -> Result<(), Error> {
    if k % 2 != 0 {
        return Err(Error::InvalidFlatBuffer);
    }
    let bytes_per_row = k / 2;
    if a.len() < m * k || w.len() < n * bytes_per_row || scales.len() < n || out.len() < m * n {
        return Err(Error::Truncated);
    }
    for row in 0..m {
        let a_row = &a[row * k..(row + 1) * k];
        for col in 0..n {
            let scale = scales[col];
            let w_row = &w[col * bytes_per_row..(col + 1) * bytes_per_row];
            let mut acc = 0.0f32;
            for (b_idx, &byte) in w_row.iter().enumerate() {
                let lo = byte & 0x0f;
                let hi = (byte >> 4) & 0x0f;
                let lo_signed = if lo < 8 { lo as f32 } else { (lo as i8 - 16) as f32 };
                let hi_signed = if hi < 8 { hi as f32 } else { (hi as i8 - 16) as f32 };
                acc += a_row[b_idx * 2] * lo_signed + a_row[b_idx * 2 + 1] * hi_signed;
            }
            out[row * n + col] = acc * scale;
        }
    }
    Ok(())
}

/// Kernel: Split-half Rotary Positional Embedding matching `odml.rope.impl`:
/// Splits head vector of size `head_dim` into two equal halves of size `half_dim = head_dim / 2`.
/// Computes theta_j = pos * base^(-2j / head_dim).
/// out[j] = x[j] * cos(theta_j) - x[j + half_dim] * sin(theta_j)
/// out[j + half_dim] = x[j] * sin(theta_j) + x[j + half_dim] * cos(theta_j)
pub fn kernel_rope_split_half(
    vec: &mut [f32],
    head_dim: usize,
    pos: usize,
    base: f32,
) -> Result<(), Error> {
    if head_dim == 0 || head_dim % 2 != 0 || vec.len() < head_dim || base <= 0.0 {
        return Err(Error::InvalidFlatBuffer);
    }
    let half_dim = head_dim / 2;
    let num_heads = vec.len() / head_dim;
    for h in 0..num_heads {
        let head = &mut vec[h * head_dim..(h + 1) * head_dim];
        for j in 0..half_dim {
            let exponent = (2 * j) as f32 / (head_dim as f32);
            let freq = 1.0f32 / crate::inference::f32_pow_approx(base, exponent);
            let theta = (pos as f32) * freq;
            let c = crate::inference::f32_cos(theta);
            let s = crate::inference::f32_sin(theta);
            let x1 = head[j];
            let x2 = head[j + half_dim];
            head[j] = x1 * c - x2 * s;
            head[j + half_dim] = x1 * s + x2 * c;
        }
    }
    Ok(())
}

/// Executes the exact Section 2 embedder graph:
/// Clamps token_id within valid range [0, 262144), gathers 512 INT4 weights
/// scaled by the per-token scale from Section 2 quantization parameters, and
/// multiplies by the Section 2 scalar multiplier (sqrt(512) = 22.6274166).
pub fn execute_section2_embedding_lookup(
    sec2_bytes: &[u8],
    sec4_weights: &[u8],
    token_id: i32,
    out: &mut [f32; 512],
) -> Result<(), Error> {
    // 1. Read lower/upper bound and multiplier from Sec 2 buffers
    let buf1 = inspect_tflite_buffer(sec2_bytes, 1)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let lower_bound = if buf1.len() >= 4 { read_u32(buf1, 0)? as i32 } else { 0 };

    let buf2 = inspect_tflite_buffer(sec2_bytes, 2)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let upper_bound = if buf2.len() >= 4 { read_u32(buf2, 0)? as i32 } else { 262144 };

    let buf3 = inspect_tflite_buffer(sec2_bytes, 3)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let fallback = if buf3.len() >= 4 { read_u32(buf3, 0)? as i32 } else { 0 };

    let buf6 = inspect_tflite_buffer(sec2_bytes, 6)?
        .ok_or(Error::InvalidFlatBuffer)?;
    let scalar_mul = if buf6.len() >= 4 {
        f32::from_bits(read_u32(buf6, 0)?)
    } else {
        22.6274166f32
    };

    // 2. Op 0..3: Clamp token_id using kernels
    let mut ge = [false; 1];
    kernel_greater_equal(&[token_id], &[lower_bound], &mut ge)?;

    let mut lt = [false; 1];
    kernel_less(&[token_id], &[upper_bound], &mut lt)?;

    let mut valid = [false; 1];
    kernel_logical_and(&ge, &lt, &mut valid)?;

    let mut clamped = [0i32; 1];
    kernel_select_v2(&valid, &[token_id], &[fallback], &mut clamped)?;

    let mut reshaped = [0i32; 1];
    kernel_reshape(&clamped, &mut reshaped)?;

    // 3. Op 5: Gather INT4 weights with scale
    let (scale_offset, num_scales) = extract_tflite_tensor_quantization_scales_offset(sec2_bytes, 0, 10)?;
    let idx = reshaped[0] as usize;
    if idx >= num_scales {
        return Err(Error::SectionRange);
    }
    let scale_bytes_offset = scale_offset.checked_add(idx.checked_mul(4).ok_or(Error::SectionRange)?)
        .ok_or(Error::SectionRange)?;
    let scale_bits = read_u32(sec2_bytes, scale_bytes_offset)?;
    let token_scale = f32::from_bits(scale_bits);

    // Unpack INT4 weights
    let bytes_per_token = 256;
    let w_start = if sec4_weights.len() == bytes_per_token {
        0
    } else {
        idx.checked_mul(bytes_per_token).ok_or(Error::SectionRange)?
    };
    let w_end = w_start.checked_add(bytes_per_token).ok_or(Error::SectionRange)?;
    if w_end > sec4_weights.len() {
        return Err(Error::SectionRange);
    }

    for (b_idx, &byte) in sec4_weights[w_start..w_end].iter().enumerate() {
        let lo = byte & 0x0f;
        let hi = (byte >> 4) & 0x0f;
        let lo_signed = if lo < 8 { lo as f32 } else { (lo as i8 - 16) as f32 };
        let hi_signed = if hi < 8 { hi as f32 } else { (hi as i8 - 16) as f32 };
        out[b_idx * 2] = lo_signed * token_scale;
        out[b_idx * 2 + 1] = hi_signed * token_scale;
    }

    // 4. Op 6: Multiply by scalar multiplier
    kernel_mul_scalar_f32(out, scalar_mul);

    Ok(())
}

/// Executes Section 3 projection head:
/// Takes the 512-D pooled token embedding vector, applies INT4 projection weight matrix
/// of shape [768, 512] with per-channel quantization scales extracted from Section 3,
/// and computes the final L2-normalized 768-D embedding vector.
pub fn execute_section3_projection_head(
    sec3_bytes: &[u8],
    sec5_head_weights: &[u8],
    pooled_512: &[f32; 512],
    out_768: &mut [f32; 768],
) -> Result<(), Error> {
    if sec5_head_weights.len() < 768 * 256 {
        return Err(Error::Truncated);
    }
    // Subgraph 1 (encoder_1x128), Tensor 1473 is the projection head weight
    let (scale_offset, num_scales) =
        extract_tflite_tensor_quantization_scales_offset(sec3_bytes, 1, 1473)?;
    if num_scales < 768 {
        return Err(Error::InvalidFlatBuffer);
    }
    let mut scales = [0.0f32; 768];
    for i in 0..768usize {
        let off = scale_offset
            .checked_add(i.checked_mul(4usize).ok_or(Error::SectionRange)?)
            .ok_or(Error::SectionRange)?;
        scales[i] = f32::from_bits(read_u32(sec3_bytes, off)?);
    }

    kernel_matmul_int4_scaled(
        pooled_512,
        &sec5_head_weights[..768 * 256],
        &scales,
        1,
        512,
        768,
        out_768,
    )?;

    // L2 Normalization
    let mut sum_sq = 0.0f32;
    for &x in out_768.iter() {
        sum_sq += x * x;
    }
    let norm = crate::f32_sqrt(sum_sq);
    if norm > 1e-9 {
        let inv_norm = 1.0f32 / norm;
        for x in out_768.iter_mut() {
            *x *= inv_norm;
        }
    }

    Ok(())
}

fn read_protobuf_varint(data: &[u8], offset: &mut usize) -> Result<u64, Error> {
    let mut res = 0u64;
    let mut shift = 0u32;
    while *offset < data.len() {
        let b = data[*offset];
        *offset += 1;
        res |= ((b & 0x7f) as u64) << shift;
        if (b & 0x80) == 0 {
            return Ok(res);
        }
        shift += 7;
        if shift > 64 {
            return Err(Error::InvalidFlatBuffer);
        }
    }
    Err(Error::Truncated)
}

fn format_byte_token(b: u8, out: &mut [u8; 6]) -> &[u8] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out[0] = b'<';
    out[1] = b'0';
    out[2] = b'x';
    out[3] = HEX[(b >> 4) as usize];
    out[4] = HEX[(b & 0x0F) as usize];
    out[5] = b'>';
    out
}

/// Pure-Rust `#![no_std]` SentencePiece Byte-Pair Encoding (BPE) tokenizer
/// parsed directly from Section 1 (`SentencePieceTokenizer`) of LiteRT-LM packages.
pub struct LiteRtLmTokenizer<'a> {
    section1_bytes: &'a [u8],
    sorted_ids: alloc::vec::Vec<u32>,
    id_to_chunk_offset: alloc::vec::Vec<u32>,
    byte_tokens: [u32; 256],
}

impl<'a> LiteRtLmTokenizer<'a> {
    /// Parses the SentencePiece Model protobuf from Section 1 bytes.
    pub fn parse(section1_bytes: &'a [u8]) -> Result<Self, Error> {
        let mut id_to_chunk_offset = alloc::vec::Vec::with_capacity(262_144);
        let mut offset = 0;
        while offset < section1_bytes.len() {
            let tag = read_protobuf_varint(section1_bytes, &mut offset)?;
            let fnum = tag >> 3;
            let wtype = (tag & 0x7) as u8;
            if wtype == 2 {
                let len = read_protobuf_varint(section1_bytes, &mut offset)? as usize;
                let chunk_start = offset;
                offset = offset.checked_add(len).ok_or(Error::SectionRange)?;
                if offset > section1_bytes.len() {
                    return Err(Error::Truncated);
                }
                if fnum == 1 {
                    id_to_chunk_offset.push(chunk_start as u32);
                }
            } else if wtype == 0 {
                let _ = read_protobuf_varint(section1_bytes, &mut offset)?;
            } else if wtype == 1 {
                offset = offset.checked_add(8).ok_or(Error::SectionRange)?;
            } else if wtype == 5 {
                offset = offset.checked_add(4).ok_or(Error::SectionRange)?;
            } else {
                return Err(Error::InvalidFlatBuffer);
            }
        }

        let token_count = id_to_chunk_offset.len();
        if token_count == 0 {
            return Err(Error::Truncated);
        }

        let mut sorted_ids: alloc::vec::Vec<u32> = (0..token_count as u32).collect();
        sorted_ids.sort_unstable_by(|&a, &b| {
            let piece_a = Self::read_piece_bytes(section1_bytes, id_to_chunk_offset[a as usize] as usize);
            let piece_b = Self::read_piece_bytes(section1_bytes, id_to_chunk_offset[b as usize] as usize);
            piece_a.cmp(piece_b)
        });

        // Initialize byte tokens <0x00>..<0xFF>
        let mut byte_tokens = [3u32; 256];
        for b in 0..=255u8 {
            let mut name_buf = [0u8; 6];
            let name = format_byte_token(b, &mut name_buf);
            if let Ok(idx) = sorted_ids.binary_search_by(|&id| {
                let p = Self::read_piece_bytes(section1_bytes, id_to_chunk_offset[id as usize] as usize);
                p.cmp(name)
            }) {
                byte_tokens[b as usize] = sorted_ids[idx];
            } else if (238 + (b as usize)) < token_count {
                byte_tokens[b as usize] = (238 + (b as u32)) as u32;
            }
        }

        Ok(Self {
            section1_bytes,
            sorted_ids,
            id_to_chunk_offset,
            byte_tokens,
        })
    }

    #[inline]
    fn read_piece_bytes(section1_bytes: &[u8], chunk_start: usize) -> &[u8] {
        if chunk_start >= section1_bytes.len() {
            return b"";
        }
        let chunk = &section1_bytes[chunk_start..];
        if chunk.is_empty() || chunk[0] != 0x0A {
            return b"";
        }
        let mut off = 1;
        let plen = match read_protobuf_varint(chunk, &mut off) {
            Ok(l) => l as usize,
            Err(_) => return b"",
        };
        if off + plen <= chunk.len() {
            &chunk[off..off + plen]
        } else {
            b""
        }
    }

    #[inline]
    fn read_score_and_type(section1_bytes: &[u8], chunk_start: usize) -> (f32, u8) {
        if chunk_start >= section1_bytes.len() {
            return (0.0, 1);
        }
        let chunk = &section1_bytes[chunk_start..];
        if chunk.is_empty() || chunk[0] != 0x0A {
            return (0.0, 1);
        }
        let mut off = 1;
        let plen = match read_protobuf_varint(chunk, &mut off) {
            Ok(l) => l as usize,
            Err(_) => return (0.0, 1),
        };
        let mut p_end = off + plen;
        let mut score = 0.0f32;
        let mut ptype = 1u8;
        while p_end < chunk.len() {
            let tag = match read_protobuf_varint(chunk, &mut p_end) {
                Ok(t) => t,
                Err(_) => break,
            };
            let fnum = tag >> 3;
            let wtype = (tag & 0x7) as u8;
            if fnum == 2 && wtype == 5 {
                if p_end + 4 <= chunk.len() {
                    score = f32::from_le_bytes([chunk[p_end], chunk[p_end + 1], chunk[p_end + 2], chunk[p_end + 3]]);
                    p_end += 4;
                } else {
                    break;
                }
            } else if fnum == 3 && wtype == 0 {
                if let Ok(t_val) = read_protobuf_varint(chunk, &mut p_end) {
                    ptype = t_val as u8;
                } else {
                    break;
                }
            } else if wtype == 0 {
                let _ = read_protobuf_varint(chunk, &mut p_end);
            } else if wtype == 1 {
                p_end += 8;
            } else if wtype == 2 {
                if let Ok(l) = read_protobuf_varint(chunk, &mut p_end) {
                    p_end += l as usize;
                } else {
                    break;
                }
            } else if wtype == 5 {
                p_end += 4;
            } else {
                break;
            }
        }
        (score, ptype)
    }

    /// Finds a piece by byte slice in $O(\log N)$ time, returning (piece_bytes, id, score, type).
    pub fn find_piece(&self, target: &[u8]) -> Option<(&'a [u8], u32, f32, u8)> {
        let idx = self.sorted_ids.binary_search_by(|&id| {
            let piece = Self::read_piece_bytes(self.section1_bytes, self.id_to_chunk_offset[id as usize] as usize);
            piece.cmp(target)
        }).ok()?;
        let id = self.sorted_ids[idx];
        let chunk_off = self.id_to_chunk_offset[id as usize] as usize;
        let piece = Self::read_piece_bytes(self.section1_bytes, chunk_off);
        let (score, ptype) = Self::read_score_and_type(self.section1_bytes, chunk_off);
        Some((piece, id, score, ptype))
    }

    /// Returns the exact piece bytes for a token ID.
    pub fn piece_of(&self, id: u32) -> &'a [u8] {
        if (id as usize) < self.id_to_chunk_offset.len() {
            Self::read_piece_bytes(self.section1_bytes, self.id_to_chunk_offset[id as usize] as usize)
        } else {
            b""
        }
    }

    /// Returns the total vocabulary size.
    pub fn vocab_size(&self) -> usize {
        self.id_to_chunk_offset.len()
    }

    /// Tokenizes `text` using SentencePiece BPE into token IDs in `out`.
    /// Returns the number of tokens written.
    pub fn encode(&self, text: &str, out: &mut [u32]) -> Result<usize, Error> {
        if text.is_empty() {
            return Ok(0);
        }
        let mut norm = alloc::vec::Vec::with_capacity(text.len() * 3 + 4);
        for c in text.chars() {
            if c == ' ' {
                norm.extend_from_slice(b"\xe2\x96\x81");
            } else {
                let mut buf = [0u8; 4];
                norm.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }

        let mut symbols: alloc::vec::Vec<&'a [u8]> = alloc::vec::Vec::new();
        let mut i = 0;
        while i < norm.len() {
            let b = norm[i];
            let char_len = if b < 0x80 {
                1
            } else if (b & 0xE0) == 0xC0 {
                2
            } else if (b & 0xF0) == 0xE0 {
                3
            } else if (b & 0xF8) == 0xF0 {
                4
            } else {
                1
            };
            let end = (i + char_len).min(norm.len());
            let char_slice = &norm[i..end];
            if let Some((p, _, _, _)) = self.find_piece(char_slice) {
                symbols.push(p);
            } else {
                for &byte_val in char_slice {
                    let b_id = self.byte_tokens[byte_val as usize];
                    symbols.push(self.piece_of(b_id));
                }
            }
            i = end;
        }

        // BPE iterative merge: repeatedly merge the adjacent pair with the highest unigram score.
        while symbols.len() >= 2 {
            let mut best_pair_idx = None;
            let mut best_score = -1e30f32;
            let mut best_merged_piece = None;

            let mut j = 0;
            while j + 1 < symbols.len() {
                let a = symbols[j];
                let b = symbols[j + 1];
                let mut merged_buf = [0u8; 128];
                if a.len() + b.len() <= merged_buf.len() {
                    merged_buf[..a.len()].copy_from_slice(a);
                    merged_buf[a.len()..a.len() + b.len()].copy_from_slice(b);
                    let merged_slice = &merged_buf[..a.len() + b.len()];
                    if let Some((merged_piece, _, score, ptype)) = self.find_piece(merged_slice) {
                        if ptype != 3 && score > best_score {
                            best_score = score;
                            best_pair_idx = Some(j);
                            best_merged_piece = Some(merged_piece);
                        }
                    }
                }
                j += 1;
            }

            if let (Some(idx), Some(merged_piece)) = (best_pair_idx, best_merged_piece) {
                symbols[idx] = merged_piece;
                symbols.remove(idx + 1);
            } else {
                break;
            }
        }

        if symbols.len() > out.len() {
            return Err(Error::SectionRange);
        }
        for (idx, &sym) in symbols.iter().enumerate() {
            let id = self.find_piece(sym).map(|p| p.1).unwrap_or(3);
            out[idx] = id;
        }
        Ok(symbols.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn one_section_container(begin: u64, end: u64) -> [u8; 128] {
        let mut bytes = [0u8; 128];
        bytes[..8].copy_from_slice(MAGIC);
        put_u32(&mut bytes, 8, 1);
        put_u32(&mut bytes, 12, 7);
        put_u64(&mut bytes, 24, 128);

        // FlatBuffer metadata begins at file byte 32. Root table at metadata
        // offset 12, SectionMetadata at 32, vector at 40, SectionObject at 64.
        let metadata = &mut bytes[32..];
        put_u32(metadata, 0, 12);
        put_u16(metadata, 4, 8);
        put_u16(metadata, 6, 12);
        put_u16(metadata, 8, 0);
        put_u16(metadata, 10, 8);
        put_u32(metadata, 12, 8);
        put_u32(metadata, 20, 12);

        put_u16(metadata, 24, 6);
        put_u16(metadata, 26, 8);
        put_u16(metadata, 28, 4);
        put_u32(metadata, 32, 8);
        put_u32(metadata, 36, 4);
        put_u32(metadata, 40, 1);
        put_u32(metadata, 44, 20);

        put_u16(metadata, 52, 12);
        put_u16(metadata, 54, 32);
        put_u16(metadata, 56, 0);
        put_u16(metadata, 58, 8);
        put_u16(metadata, 60, 16);
        put_u16(metadata, 62, 24);
        put_u32(metadata, 64, 12);
        put_u64(metadata, 72, begin);
        put_u64(metadata, 80, end);
        metadata[88] = SectionType::TfLiteModel as u8;
        bytes
    }

    #[test]
    fn accepts_bounded_aligned_section_inventory() {
        let bytes = one_section_container(BLOCK_SIZE, BLOCK_SIZE + 123);
        let result = inspect(&bytes, BLOCK_SIZE + 123).expect("valid package");
        assert_eq!(
            result.envelope.version,
            Version {
                major: 1,
                minor: 7,
                patch: 0
            }
        );
        assert_eq!(result.section_count, 1);
    }

    #[test]
    fn distinguishes_magic_version_and_truncation_failures() {
        let mut bytes = one_section_container(BLOCK_SIZE, BLOCK_SIZE + 1);
        bytes[0] = b'X';
        assert_eq!(inspect(&bytes, BLOCK_SIZE + 1), Err(Error::BadMagic));
        bytes[0] = b'L';
        put_u32(&mut bytes, 8, 2);
        assert_eq!(
            inspect(&bytes, BLOCK_SIZE + 1),
            Err(Error::UnsupportedMajor)
        );
        assert_eq!(parse_envelope(&bytes[..20], 100), Err(Error::Truncated));
    }

    #[test]
    fn rejects_unaligned_overlapping_or_out_of_file_sections() {
        let unaligned = one_section_container(BLOCK_SIZE + 1, BLOCK_SIZE + 2);
        assert_eq!(
            inspect(&unaligned, BLOCK_SIZE + 2),
            Err(Error::SectionUnaligned)
        );
        let past_file = one_section_container(BLOCK_SIZE, BLOCK_SIZE + 10);
        assert_eq!(
            inspect(&past_file, BLOCK_SIZE + 9),
            Err(Error::SectionRange)
        );
    }

    #[test]
    fn rejects_fields_outside_declared_flatbuffer_object() {
        let mut bytes = one_section_container(BLOCK_SIZE, BLOCK_SIZE + 1);
        // The section vtable declares an object only four bytes long, but its
        // begin and end offsets point well beyond that object.
        put_u16(&mut bytes, HEADER_BEGIN + 54, 4);
        assert_eq!(
            inspect(&bytes, BLOCK_SIZE + 1),
            Err(Error::InvalidFlatBuffer)
        );

        let mut bytes = one_section_container(BLOCK_SIZE, BLOCK_SIZE + 1);
        // End offset starts inside the object but its eight-byte scalar crosses
        // the declared object boundary.
        put_u16(&mut bytes, HEADER_BEGIN + 60, 28);
        assert_eq!(
            inspect(&bytes, BLOCK_SIZE + 1),
            Err(Error::InvalidFlatBuffer)
        );

        let mut bytes = one_section_container(BLOCK_SIZE, BLOCK_SIZE + 1);
        // A vtable cannot extend past the supplied metadata.
        put_u16(&mut bytes, HEADER_BEGIN + 52, 200);
        assert_eq!(
            inspect(&bytes, BLOCK_SIZE + 1),
            Err(Error::InvalidFlatBuffer)
        );
    }

    #[test]
    fn validates_real_package_header_when_configured() {
        extern crate std as host_std;
        use host_std::io::{Read, Seek, SeekFrom};

        let Ok(path) = host_std::env::var("INTEROS_REAL_LITERTLM_PATH") else {
            return;
        };
        let mut file = host_std::fs::File::open(path).expect("open configured LiteRT-LM package");
        let file_size = file.metadata().expect("read package metadata").len();
        let mut envelope_bytes = [0u8; ENVELOPE_BYTES];
        file.read_exact(&mut envelope_bytes)
            .expect("read package envelope");
        let envelope = parse_envelope(&envelope_bytes, file_size).expect("parse package envelope");
        let mut header = host_std::vec![0u8; envelope.header_end];
        file.seek(SeekFrom::Start(0)).expect("rewind package");
        file.read_exact(&mut header).expect("read package header");
        let inventory = inspect(&header, file_size).expect("validate package section inventory");
        assert!(inventory.section_count > 0);
    }

    #[test]
    fn parse_sections_enumerates_sections() {
        let bytes = one_section_container(BLOCK_SIZE, BLOCK_SIZE + 123);
        let mut count = 0;
        let mut last_type = SectionType::None;
        let env = parse_sections(&bytes, BLOCK_SIZE + 123, |_idx, sec| {
            count += 1;
            last_type = sec.data_type;
            Ok(())
        })
        .expect("parse sections");
        assert_eq!(count, 1);
        assert_eq!(last_type, SectionType::TfLiteModel);
        assert_eq!(env.header_end, 128);
    }

    #[test]
    fn extracts_tflite_tensor_plan_from_real_package_when_configured() {
        extern crate std as host_std;
        use host_std::io::{Read, Seek, SeekFrom};

        let Ok(path) = host_std::env::var("INTEROS_REAL_LITERTLM_PATH") else {
            return;
        };
        let mut file = host_std::fs::File::open(path).expect("open configured LiteRT-LM package");
        let file_size = file.metadata().expect("read package metadata").len();
        let mut envelope_bytes = [0u8; ENVELOPE_BYTES];
        file.read_exact(&mut envelope_bytes)
            .expect("read package envelope");
        let envelope = parse_envelope(&envelope_bytes, file_size).expect("parse package envelope");
        let mut header = host_std::vec![0u8; envelope.header_end];
        file.seek(SeekFrom::Start(0)).expect("rewind package");
        file.read_exact(&mut header).expect("read package header");

        let mut tflite_sections = host_std::vec![];
        parse_sections(&header, file_size, |_idx, sec| {
            if sec.data_type == SectionType::TfLiteModel {
                tflite_sections.push(sec);
            }
            Ok(())
        })
        .expect("parse sections");

        assert!(!tflite_sections.is_empty(), "expected at least 1 TfLiteModel section");
        let sec0 = &tflite_sections[0];
        let sec_len = (sec0.end - sec0.begin) as usize;
        let mut model_bytes = host_std::vec![0u8; sec_len];
        file.seek(SeekFrom::Start(sec0.begin)).expect("seek to tflite section");
        file.read_exact(&mut model_bytes).expect("read tflite model bytes");

        let model_info = inspect_tflite_model(&model_bytes).expect("inspect tflite model");
        assert_eq!(model_info.version, 3);
        assert!(model_info.subgraphs_count >= 1);

        let sg0 = inspect_tflite_subgraph(&model_bytes, 0).expect("inspect subgraph 0");
        assert!(sg0.tensor_count > 0);
        assert!(sg0.operator_count > 0);

        let t0 = inspect_tflite_tensor(&model_bytes, 0, 0).expect("inspect tensor 0");
        assert_eq!(t0.tensor_type, 2); // INT32 token ids

        let mut shape = [0i32; 4];
        let rank = inspect_tflite_tensor_shape(&model_bytes, 0, 0, &mut shape).expect("inspect shape");
        assert_eq!(rank, 2);
        assert_eq!(&shape[..rank], &[1, 1]);

        let opcode0 = inspect_tflite_operator_code(&model_bytes, 0).expect("inspect opcode 0");
        assert!(opcode0 >= 0);

        let mut in_idx = [0i32; 8];
        let mut out_idx = [0i32; 8];
        let (num_in, num_out) = inspect_tflite_operator_io(&model_bytes, 0, 0, &mut in_idx, &mut out_idx)
            .expect("inspect op io");
        assert!(num_in >= 1);
        assert!(num_out >= 1);
    }

    #[test]
    fn test_pure_rust_comparison_and_logical_kernels() {
        let a = [10, -5, 0, 100];
        let mut ge = [false; 4];
        kernel_greater_equal(&a, &[0], &mut ge).expect("ge");
        assert_eq!(ge, [true, false, true, true]);

        let mut lt = [false; 4];
        kernel_less(&a, &[50], &mut lt).expect("lt");
        assert_eq!(lt, [true, true, true, false]);

        let mut and_out = [false; 4];
        kernel_logical_and(&ge, &lt, &mut and_out).expect("and");
        assert_eq!(and_out, [true, false, true, false]);

        let mut select_out = [0i32; 4];
        kernel_select_v2(&and_out, &a, &[999], &mut select_out).expect("select");
        assert_eq!(select_out, [10, 999, 0, 999]);
    }

    #[test]
    fn test_pure_rust_reshape_and_scalar_mul_kernels() {
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut reshaped = [0.0f32; 4];
        kernel_reshape(&input, &mut reshaped).expect("reshape");
        assert_eq!(reshaped, input);

        kernel_mul_scalar_f32(&mut reshaped, 2.5);
        assert_eq!(reshaped, [2.5, 5.0, 7.5, 10.0]);
    }

    #[test]
    fn test_pure_rust_gather_bf16_to_f32() {
        // 1.0f32 in bfloat16 is 0x3f80
        // 2.0f32 in bfloat16 is 0x4000
        let weights = [0x3f80u16, 0x4000u16, 0xbf80u16, 0xc000u16];
        let mut out = [0.0f32; 4];
        kernel_gather_bfloat16_to_f32(&[0, 1], &weights, 2, &mut out).expect("gather bf16");
        assert_eq!(out, [1.0, 2.0, -1.0, -2.0]);
    }

    #[test]
    fn test_pure_rust_gather_int4_scaled() {
        // 2 tokens, 4 dims each (2 bytes per token)
        // Token 0: byte0: lo=1, hi=2; byte1: lo=7, hi=8 (-8)
        // Token 1: byte0: lo=15 (-1), hi=0; byte1: lo=9 (-7), hi=6
        let weights = [0x21u8, 0x87u8, 0x0fu8, 0x69u8];
        let scales = [0.5f32, 2.0f32];
        let mut out = [0.0f32; 8];
        kernel_gather_int4_scaled(&[0, 1], &weights, &scales, 4, &mut out).expect("gather int4");
        // Token 0: [1 * 0.5, 2 * 0.5, 7 * 0.5, -8 * 0.5] = [0.5, 1.0, 3.5, -4.0]
        // Token 1: [-1 * 2.0, 0 * 2.0, -7 * 2.0, 6 * 2.0] = [-2.0, 0.0, -14.0, 12.0]
        assert_eq!(out[0], 0.5);
        assert_eq!(out[1], 1.0);
        assert_eq!(out[2], 3.5);
        assert_eq!(out[3], -4.0);
        assert_eq!(out[4], -2.0);
        assert_eq!(out[5], 0.0);
        assert_eq!(out[6], -14.0);
        assert_eq!(out[7], 12.0);
    }

    #[test]
    fn test_execute_section2_embedding_lookup_if_model_present() {
        extern crate std as rust_std;
        let model_path = "models/downloads/embeddinggemma2-text/embeddinggemma-2-text-270m.litertlm";
        let file_bytes = match rust_std::fs::read(model_path) {
            Ok(b) => b,
            Err(_) => return, // optional on systems where model not downloaded
        };
        let mut sec2_range = (0usize, 0usize);
        let mut sec4_range = (0usize, 0usize);
        parse_sections(&file_bytes[..65536.min(file_bytes.len())], file_bytes.len() as u64, |idx, sec| {
            if idx == 2 {
                sec2_range = (sec.begin as usize, sec.end as usize);
            } else if idx == 4 {
                sec4_range = (sec.begin as usize, sec.end as usize);
            }
            Ok(())
        }).expect("parse sections");

        let sec2_bytes = &file_bytes[sec2_range.0..sec2_range.1];
        let sec4_weights = &file_bytes[sec4_range.0..sec4_range.1];

        // Execute token 23391 ("hello")
        let mut out = [0.0f32; 512];
        execute_section2_embedding_lookup(sec2_bytes, sec4_weights, 23391, &mut out)
            .expect("lookup token 23391");

        let mut sum_sq = 0.0f32;
        for &x in &out {
            sum_sq += x * x;
        }
        let norm = sum_sq.sqrt();
        assert!((norm - 22.48).abs() < 0.1, "Expected norm ~22.48, got {}", norm);

        // Out of bounds token clamps to token 0
        let mut out_oob = [0.0f32; 512];
        execute_section2_embedding_lookup(sec2_bytes, sec4_weights, 999_999, &mut out_oob)
            .expect("lookup oob token");
        let mut out_0 = [0.0f32; 512];
        execute_section2_embedding_lookup(sec2_bytes, sec4_weights, 0, &mut out_0)
            .expect("lookup token 0");
        assert_eq!(out_oob, out_0);
    }

    #[test]
    fn test_package_backed_sentencepiece_bpe_tokenizer() {
        extern crate std as rust_std;
        let model_path = "models/downloads/embeddinggemma2-text/embeddinggemma-2-text-270m.litertlm";
        let file_bytes = match rust_std::fs::read(model_path) {
            Ok(b) => b,
            Err(_) => return, // optional on systems where model not downloaded
        };

        let mut sec1_range = (0usize, 0usize);
        parse_sections(&file_bytes[..65536.min(file_bytes.len())], file_bytes.len() as u64, |idx, sec| {
            if idx == 1 {
                sec1_range = (sec.begin as usize, sec.end as usize);
            }
            Ok(())
        }).expect("parse sections");

        assert!(sec1_range.1 > sec1_range.0);
        let sec1_bytes = &file_bytes[sec1_range.0..sec1_range.1];
        let tokenizer = LiteRtLmTokenizer::parse(sec1_bytes).expect("parse tokenizer");
        assert_eq!(tokenizer.vocab_size(), 262_144);

        // Verify exact token IDs requested by Section 11 of the specification:
        let mut tokens = [0u32; 64];

        // 1. "hello" -> [23391]
        let n = tokenizer.encode("hello", &mut tokens).expect("encode hello");
        assert_eq!(&tokens[..n], &[23391]);

        // 2. "InterOS" -> [3845, 3354]
        let n = tokenizer.encode("InterOS", &mut tokens).expect("encode InterOS");
        assert_eq!(&tokens[..n], &[3845, 3354]);

        // 3. "local AI" -> [6047, 12498]
        let n = tokenizer.encode("local AI", &mut tokens).expect("encode local AI");
        assert_eq!(&tokens[..n], &[6047, 12498]);

        // 4. "semantic search" -> [108414, 3927]
        let n = tokenizer.encode("semantic search", &mut tokens).expect("encode semantic search");
        assert_eq!(&tokens[..n], &[108414, 3927]);

        // 5. "rent receipt" -> [1344, 19971]
        let n = tokenizer.encode("rent receipt", &mut tokens).expect("encode rent receipt");
        assert_eq!(&tokens[..n], &[1344, 19971]);

        // 6. "April housing payment" -> [30651, 11132, 7548]
        let n = tokenizer.encode("April housing payment", &mut tokens).expect("encode April housing payment");
        assert_eq!(&tokens[..n], &[30651, 11132, 7548]);

        // 7. "capability security" -> [230811, 5052]
        let n = tokenizer.encode("capability security", &mut tokens).expect("encode capability security");
        assert_eq!(&tokens[..n], &[230811, 5052]);

        // 8. "offline smartphone" -> [109076, 15711]
        let n = tokenizer.encode("offline smartphone", &mut tokens).expect("encode offline smartphone");
        assert_eq!(&tokens[..n], &[109076, 15711]);

        // 9. "private local assistant" -> [7128, 2263, 16326]
        let n = tokenizer.encode("private local assistant", &mut tokens).expect("encode private local assistant");
        assert_eq!(&tokens[..n], &[7128, 2263, 16326]);

        // Edge cases:
        // Empty string
        let n = tokenizer.encode("", &mut tokens).expect("encode empty");
        assert_eq!(n, 0);

        // Whitespace
        let n = tokenizer.encode("   ", &mut tokens).expect("encode whitespace");
        assert_eq!(&tokens[..n], &[242]);

        // Punctuation
        let n = tokenizer.encode("hello, world!", &mut tokens).expect("encode punctuation");
        assert_eq!(&tokens[..n], &[23391, 235284, 108, 235341]);

        // Decode round-trip verification
        assert_eq!(tokenizer.piece_of(23391), b"hello");
        assert_eq!(tokenizer.piece_of(1344), b"rent");
        assert_eq!(tokenizer.piece_of(19971), b"\xe2\x96\x81receipt");
    }

    #[test]
    fn test_kernel_rms_norm_numerical_precision_and_bounds() {
        // Deterministic vector [1.0, 2.0, 3.0, 4.0] with identity weights
        let mut x = [1.0f32, 2.0f32, 3.0f32, 4.0f32];
        let w = [1.0f32, 1.0f32, 1.0f32, 1.0f32];
        let eps = 1e-6f32;
        kernel_rms_norm(&mut x, &w, eps).expect("rms_norm");

        // Mean square = (1 + 4 + 9 + 16) / 4 = 7.5. Scale = 1 / sqrt(7.5) ~ 0.36514837
        let expected_scale = 1.0f32 / crate::f32_sqrt(7.5f32 + eps);
        assert!((x[0] - 1.0f32 * expected_scale).abs() < 1e-5);
        assert!((x[1] - 2.0f32 * expected_scale).abs() < 1e-5);
        assert!((x[2] - 3.0f32 * expected_scale).abs() < 1e-5);
        assert!((x[3] - 4.0f32 * expected_scale).abs() < 1e-5);

        // Sum of squares of normalized vector should equal dim = 4.0
        let norm_sq = x[0] * x[0] + x[1] * x[1] + x[2] * x[2] + x[3] * x[3];
        assert!((norm_sq - 4.0f32).abs() < 1e-4);

        // Malformed inputs:
        let mut empty: [f32; 0] = [];
        assert!(kernel_rms_norm(&mut empty, &w, eps).is_err());
        let insufficient_w = [1.0f32];
        assert!(kernel_rms_norm(&mut x, &insufficient_w, eps).is_err());
        assert!(kernel_rms_norm(&mut x, &w, -1.0).is_err());
    }

    #[test]
    fn test_kernel_gelu_numerical_tanh_accuracy() {
        let mut vals = [0.0f32, 1.0f32, -1.0f32, 10.0f32, -10.0f32];
        kernel_gelu(&mut vals);

        // GELU(0) = 0
        assert_eq!(vals[0], 0.0);
        // GELU(1.0) ~ 0.841192
        assert!((vals[1] - 0.841192).abs() < 1e-4, "GELU(1.0) = {}", vals[1]);
        // GELU(-1.0) ~ -0.158808
        assert!((vals[2] - (-0.158808)).abs() < 1e-4, "GELU(-1.0) = {}", vals[2]);
        // Asymptotes:
        assert!((vals[3] - 10.0).abs() < 1e-3);
        assert_eq!(vals[4], 0.0);
    }

    #[test]
    fn test_kernel_softmax_stability_and_sum_to_one() {
        // Large values test numerical stability (no overflow to Inf or NaN)
        let mut large = [1000.0f32, 1001.0f32, 1002.0f32];
        kernel_softmax(&mut large).expect("softmax");
        let sum_large: f32 = large.iter().sum();
        assert!((sum_large - 1.0f32).abs() < 1e-5, "Softmax sum = {}", sum_large);
        assert!(large[2] > large[1] && large[1] > large[0]);

        // Uniform inputs
        let mut uniform = [5.0f32, 5.0f32, 5.0f32, 5.0f32];
        kernel_softmax(&mut uniform).expect("softmax");
        for &p in &uniform {
            assert!((p - 0.25f32).abs() < 1e-6);
        }

        let mut empty: [f32; 0] = [];
        assert!(kernel_softmax(&mut empty).is_err());
    }

    #[test]
    fn test_kernel_matmul_f32_deterministic_and_dimensions() {
        // A is [2, 3], B is [2, 3] (so B^T is [3, 2]), out is [2, 2]
        let a = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let b = [1.0f32, 0.0, -1.0, 2.0, 1.0, 0.0];
        let mut out = [0.0f32; 4];
        kernel_matmul_f32(&a, &b, 2, 3, 2, &mut out).expect("matmul");

        // Expected out:
        // [0, 0] = 1*1 + 2*0 + 3*(-1) = -2
        // [0, 1] = 1*2 + 2*1 + 3*0 = 4
        // [1, 0] = 4*1 + 5*0 + 6*(-1) = -2
        // [1, 1] = 4*2 + 5*1 + 6*0 = 13
        assert_eq!(out, [-2.0, 4.0, -2.0, 13.0]);

        // Bounds error
        let mut small_out = [0.0f32; 2];
        assert!(kernel_matmul_f32(&a, &b, 2, 3, 2, &mut small_out).is_err());
    }

    #[test]
    fn test_kernel_matmul_int4_scaled_signed_nibbles_and_scale() {
        // M=1, K=4, N=2
        // A = [1.0, -2.0, 3.0, -1.0]
        let a = [1.0f32, -2.0, 3.0, -1.0];
        // Row 0: byte 0 = 0xb3 (lo=3, hi=-5), byte 1 = 0x87 (lo=7, hi=-8)
        // Row 1: byte 0 = 0x0f (lo=-1, hi=0), byte 1 = 0x70 (lo=0, hi=7)
        let w = [0xb3u8, 0x87, 0x0f, 0x70];
        let scales = [0.5f32, 0.25f32];
        let mut out = [0.0f32; 2];

        kernel_matmul_int4_scaled(&a, &w, &scales, 1, 4, 2, &mut out).expect("matmul_int4");

        // Acc 0 = 1*3 + (-2)*(-5) + 3*7 + (-1)*(-8) = 3 + 10 + 21 + 8 = 42.0. Out = 42 * 0.5 = 21.0
        // Acc 1 = 1*(-1) + (-2)*0 + 3*0 + (-1)*7 = -1 + 0 + 0 - 7 = -8.0. Out = -8 * 0.25 = -2.0
        assert_eq!(out, [21.0, -2.0]);

        // Odd K is refused
        assert!(kernel_matmul_int4_scaled(&a, &w, &scales, 1, 3, 2, &mut out).is_err());
    }

    #[test]
    fn test_kernel_rope_split_half_rotation_properties() {
        // Head dim 4, 1 head, base 8000.0
        let mut vec = [1.0f32, 2.0, 3.0, 4.0];
        // Pos 0: theta = 0, so cos=1, sin=0 -> exact identity
        kernel_rope_split_half(&mut vec, 4, 0, 8000.0).expect("rope");
        assert_eq!(vec, [1.0, 2.0, 3.0, 4.0]);

        // Pos 1: rotation must preserve vector L2 norm within software sin/cos precision
        let norm_before = vec[0] * vec[0] + vec[1] * vec[1] + vec[2] * vec[2] + vec[3] * vec[3];
        kernel_rope_split_half(&mut vec, 4, 1, 8000.0).expect("rope");
        let norm_after = vec[0] * vec[0] + vec[1] * vec[1] + vec[2] * vec[2] + vec[3] * vec[3];
        assert!((norm_before - norm_after).abs() < 1e-3, "Norm before {}, after {}", norm_before, norm_after);

        // Odd head dim is refused
        assert!(kernel_rope_split_half(&mut vec, 3, 1, 8000.0).is_err());
    }

    #[test]
    fn test_execute_section3_projection_head_if_model_present() {
        extern crate std as rust_std;
        let path = "models/downloads/embeddinggemma2-text/embeddinggemma-2-text-270m.litertlm";
        if rust_std::fs::metadata(path).is_err() {
            return;
        }

        use rust_std::io::{Read, Seek, SeekFrom};
        let mut file = rust_std::fs::File::open(path).expect("open model");

        // Section 3: begin = 7897088, end = 29147808, size = 21250720
        file.seek(SeekFrom::Start(7897088)).expect("seek sec3");
        let mut sec3_bytes = alloc::vec::Vec::new();
        sec3_bytes.resize(21250720, 0u8);
        file.read_exact(&mut sec3_bytes).expect("read sec3");

        // Section 5 head weights (Weight 435): offset in Sec 5 = 68157440, size = 196608
        // Sec 5 begin = 96272384
        file.seek(SeekFrom::Start(96272384 + 68157440)).expect("seek sec5 head");
        let mut head_weights = [0u8; 196608];
        file.read_exact(&mut head_weights).expect("read head weights");

        let mut dummy_512 = [0.0f32; 512];
        let inv_sqrt_512 = 1.0f32 / crate::f32_sqrt(512.0);
        dummy_512.fill(inv_sqrt_512);

        let mut out_768 = [0.0f32; 768];
        execute_section3_projection_head(&sec3_bytes, &head_weights, &dummy_512, &mut out_768)
            .expect("execute projection head");

        // Verify output properties: finite, non-zero, L2 norm == 1.0
        let mut sum_sq = 0.0f32;
        for &x in out_768.iter() {
            assert!(x.is_finite());
            sum_sq += x * x;
        }
        let norm = crate::f32_sqrt(sum_sq);
        assert!((norm - 1.0f32).abs() < 1e-4, "Expected unit L2 norm, got {}", norm);

        // Deterministic check on first few values
        assert!((out_768[0] - 0.060137).abs() < 1e-4);
        assert!((out_768[1] - (-0.001081)).abs() < 1e-4);
    }
}
