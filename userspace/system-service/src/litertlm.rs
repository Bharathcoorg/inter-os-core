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
}
