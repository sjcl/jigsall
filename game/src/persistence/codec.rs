use super::*;
use crate::checkpoint::{SnapshotPieceState, SNAPSHOT_PLACED};
use crate::resources::pieces::MAX_Z;
use bevy::math::{UVec2, Vec2};
use jigsall_core::{PuzzleDefinition, GENERATOR_VERSION};
use sha2::{Digest, Sha256};

pub const SAVE_FORMAT_VERSION: u16 = 1;
pub const PUZIMG_FORMAT_VERSION: u16 = 1;
const SAVE_MAGIC: &[u8; 8] = b"PUZSAVE\0";
const IMAGE_MAGIC: &[u8; 8] = b"PUZIMG\0\0";
/// Maximum prefix needed for the save header. Independent of piece count.
pub const MAX_SAVE_HEADER_BYTES: usize = 190 + MAX_SAVE_TITLE_CHARS * 4;
pub(crate) const MAX_SAVE_BYTES: u64 = 222 + 320 + 16 * 1_000_000;
pub(crate) const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024 + 50;
pub fn image_hash(bytes: &[u8]) -> ImageHash {
    ImageHash(Sha256::digest(bytes).into())
}

/// Lightweight preview. The header has its own checksum; piece integrity and
/// the non-authoritative placed-count cache are verified only on full decode.
#[derive(Clone, Debug)]
pub struct SaveHeader {
    pub metadata: SaveMetadata,
    pub image_hash: ImageHash,
    pub definition: PuzzleDefinition,
    pub piece_count: usize,
    /// Non-authoritative cache, checked against piece states on full decode.
    pub placed_count: usize,
    next_z_order: u32,
    states_offset: usize,
}

/// Explicit little-endian codec; no Rust memory layout or serde wire dependency.
pub struct SaveCodec;
impl SaveCodec {
    pub fn encode(save: &PuzzleSave) -> Result<Vec<u8>, SaveError> {
        save.checkpoint.validate()?;
        let m = &save.metadata;
        validate_metadata(m)?;
        let c = &save.checkpoint;
        let d = &c.definition;
        let header_len = 190 + m.title.as_str().len();
        let total_len = header_len + c.pieces.len() * 16 + 32;
        let mut out = Vec::with_capacity(total_len);
        out.extend_from_slice(SAVE_MAGIC);
        out.extend_from_slice(&SAVE_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(header_len as u32).to_le_bytes());
        out.extend_from_slice(&(total_len as u64).to_le_bytes());
        out.extend_from_slice(&m.id.0.to_le_bytes());
        for n in [m.revision, m.created_at, m.updated_at] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        out.extend_from_slice(&(m.title.as_str().len() as u32).to_le_bytes());
        out.extend_from_slice(m.title.as_str().as_bytes());
        out.extend_from_slice(&c.image_hash.0);
        out.extend_from_slice(&d.generator_version.to_le_bytes());
        out.extend_from_slice(&d.seed.to_le_bytes());
        for n in [
            d.grid_size.x,
            d.grid_size.y,
            d.image_size.x,
            d.image_size.y,
            d.snap_distance.to_bits(),
            c.next_z_order,
            c.pieces.len() as u32,
            c.pieces
                .iter()
                .filter(|p| p.flags & SNAPSHOT_PLACED != 0)
                .count() as u32,
        ] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        out.push(u8::from(m.is_autosave));
        out.extend_from_slice(&m.game_id.0.to_le_bytes());
        out.push(u8::from(d.rotation_enabled));
        out.extend_from_slice(&image_hash(&out).0);
        for p in &c.pieces {
            for n in [
                p.position.x.to_bits(),
                p.position.y.to_bits(),
                p.z_order,
                p.flags,
            ] {
                out.extend_from_slice(&n.to_le_bytes());
            }
        }
        out.extend_from_slice(&image_hash(&out).0);
        Ok(out)
    }
    /// Decode a bounded prefix plus the actual blob size without allocating any
    /// piece records or connectivity structures.
    pub fn decode_header(prefix: &[u8], file_len: u64) -> Result<SaveHeader, SaveError> {
        let bad = SaveError::CorruptSave;
        let mut r = Reader::new(&prefix[..prefix.len().min(MAX_SAVE_HEADER_BYTES)], bad);
        read_save_version(&mut r)?;
        if !(222..=MAX_SAVE_BYTES).contains(&file_len) {
            return Err(bad("Invalid length"));
        }
        let header_len = r.u32()? as usize;
        let declared_len = r.u64()?;
        if !(190..=MAX_SAVE_HEADER_BYTES).contains(&header_len) || declared_len != file_len {
            return Err(bad("Invalid header or file length"));
        }
        let header = prefix
            .get(..header_len)
            .ok_or_else(|| bad("Truncated header"))?;
        if image_hash(&header[..header_len - 32]).0 != header[header_len - 32..] {
            return Err(bad("Header checksum mismatch"));
        }
        r.bytes = &header[..header_len - 32];
        let id = SaveId(u128::from_le_bytes(r.array()?));
        let revision = r.u64()?;
        let created_at = r.u64()?;
        let updated_at = r.u64()?;
        let title_len = r.u32()? as usize;
        if title_len > MAX_SAVE_TITLE_CHARS * 4 {
            return Err(bad("Invalid title length"));
        }
        let title =
            std::str::from_utf8(r.take(title_len)?).map_err(|_| bad("Invalid UTF-8 title"))?;
        let mut metadata = SaveMetadata {
            id,
            game_id: GameId(0),
            revision,
            created_at,
            updated_at,
            title: SaveTitle::new(title)?,
            is_autosave: false,
        };
        validate_metadata(&metadata)?;
        let hash = ImageHash(r.array()?);
        let generator_version = r.u16()?;
        if generator_version != GENERATOR_VERSION {
            return Err(SaveError::UnsupportedGenerator(generator_version));
        }
        let mut definition = PuzzleDefinition {
            generator_version,
            seed: r.u64()?,
            grid_size: UVec2::new(r.u32()?, r.u32()?),
            image_size: UVec2::new(r.u32()?, r.u32()?),
            snap_distance: f32::from_bits(r.u32()?),
            rotation_enabled: false,
        };
        definition
            .validate()
            .map_err(|_| bad("Invalid puzzle definition"))?;
        let next_z_order = r.u32()?;
        let count = r.u32()? as usize;
        let placed_count = r.u32()? as usize;
        metadata.is_autosave = match r.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err(bad("Invalid autosave flag")),
        };
        metadata.game_id = GameId(u128::from_le_bytes(r.array()?));
        definition.rotation_enabled = match r.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err(bad("Invalid rotation flag")),
        };
        let states_offset = header_len;
        if count != definition.piece_count()
            || file_len != states_offset as u64 + count as u64 * 16 + 32
            || placed_count > count
            || next_z_order < count as u32
            || next_z_order > MAX_Z
            || r.remaining() != 0
        {
            return Err(bad("Invalid piece count, summary or length"));
        }
        Ok(SaveHeader {
            metadata,
            image_hash: hash,
            definition,
            next_z_order,
            piece_count: count,
            placed_count,
            states_offset,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<PuzzleSave, SaveError> {
        let bad = SaveError::CorruptSave;
        read_save_version(&mut Reader::new(bytes, bad))?;
        if bytes.len() < 222 || bytes.len() as u64 > MAX_SAVE_BYTES {
            return Err(bad("Invalid length"));
        }
        let end = bytes.len() - 32;
        if image_hash(&bytes[..end]).0 != bytes[end..] {
            return Err(bad("Checksum mismatch"));
        }
        let header = Self::decode_header(bytes, bytes.len() as u64)?;
        let mut r = Reader::new(&bytes[header.states_offset..end], bad);
        // Count and exact state length were checked before allocation.
        let mut pieces = Vec::with_capacity(header.piece_count);
        for _ in 0..header.piece_count {
            pieces.push(SnapshotPieceState {
                position: Vec2::new(f32::from_bits(r.u32()?), f32::from_bits(r.u32()?)),
                z_order: r.u32()?,
                flags: r.u32()?,
            });
        }
        if header.placed_count
            != pieces
                .iter()
                .filter(|p| p.flags & SNAPSHOT_PLACED != 0)
                .count()
        {
            return Err(bad("Placed count cache mismatch"));
        }
        let checkpoint = PuzzleCheckpoint {
            image_hash: header.image_hash,
            definition: header.definition,
            next_z_order: header.next_z_order,
            pieces,
        };
        checkpoint.validate()?;
        Ok(PuzzleSave {
            metadata: header.metadata,
            checkpoint,
        })
    }
}
fn read_save_version(r: &mut Reader<'_>) -> Result<u16, SaveError> {
    if r.take(8)? != SAVE_MAGIC {
        return Err(SaveError::CorruptSave("Magic mismatch"));
    }
    let version = r.u16()?;
    if version != SAVE_FORMAT_VERSION {
        return Err(SaveError::UnsupportedSaveFormat(version));
    }
    Ok(version)
}
fn validate_metadata(m: &SaveMetadata) -> Result<(), SaveError> {
    if m.revision == 0 || m.updated_at < m.created_at {
        return Err(SaveError::CorruptSave("Invalid metadata"));
    }
    Ok(())
}

pub struct PuzImage;
impl PuzImage {
    pub fn encode(payload: &[u8]) -> Result<Vec<u8>, SaveError> {
        if payload.len() as u64 > MAX_IMAGE_BYTES - 50 {
            return Err(SaveError::CorruptImage("Image is too large"));
        }
        let mut out = Vec::with_capacity(50 + payload.len());
        out.extend_from_slice(IMAGE_MAGIC);
        out.extend_from_slice(&PUZIMG_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&image_hash(payload).0);
        out.extend_from_slice(payload);
        Ok(out)
    }
    /// Returns only verified original bytes, with no image re-encoding or extraction file.
    pub fn decode(bytes: &[u8], key_hash: ImageHash) -> Result<&[u8], SaveError> {
        let bad = SaveError::CorruptImage;
        let mut r = Reader::new(bytes, bad);
        if r.take(8)? != IMAGE_MAGIC {
            return Err(bad("Magic mismatch"));
        }
        let version = r.u16()?;
        if version != PUZIMG_FORMAT_VERSION {
            return Err(SaveError::UnsupportedImageFormat(version));
        }
        let len = r.u64()?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES || len != r.remaining().saturating_sub(32) as u64 {
            return Err(bad("Invalid payload length"));
        }
        let hash = ImageHash(r.array()?);
        let payload = r.take(r.remaining())?;
        if image_hash(payload) != hash {
            return Err(bad("Payload hash mismatch"));
        }
        if hash != key_hash {
            return Err(bad("Storage key hash mismatch"));
        }
        Ok(payload)
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    bad: fn(&'static str) -> SaveError,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], bad: fn(&'static str) -> SaveError) -> Self {
        Self { bytes, pos: 0, bad }
    }
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8], SaveError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| (self.bad)("Length overflow"))?;
        let out = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| (self.bad)("Truncated data"))?;
        self.pos = end;
        Ok(out)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], SaveError> {
        self.take(N)?
            .try_into()
            .map_err(|_| (self.bad)("Truncated data"))
    }
    fn u16(&mut self) -> Result<u16, SaveError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, SaveError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, SaveError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}
