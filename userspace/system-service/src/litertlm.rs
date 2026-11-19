// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Allocation-free inspection of the LiteRT-LM container envelope and section
//! table.
//!
//! This module intentionally stops at the package boundary.  A valid container
//! is not evidence that InterOS can execute its TFLite graphs.  Execution also
//! requires native Rust graph, tokenizer, operator and quantization support.

pub const MAGIC: &[u8; 8] = b"LITERTLM";
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

/// Validates the package envelope, FlatBuffer section table and every payload
/// range visible in the supplied header.
///
/// `header` must contain bytes `[0, envelope.header_end)`.  Callers can read
/// just that bounded prefix; model payloads are never copied into memory.
pub fn inspect(header: &[u8], file_size: u64) -> Result<Inspection, Error> {
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
    }

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
}
