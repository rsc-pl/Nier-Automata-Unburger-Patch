use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

struct ZipEntry {
    name: String,
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    offset: u32,
}

pub fn write_patch_zip(
    source_dir: &Path,
    zip_path: &Path,
    prefix: Option<&str>,
    patch_version: Option<&str>,
) -> Result<(), String> {
    let mut files = collect_files(source_dir)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("No files found in {}.", source_dir.display()));
    }

    if let Some(parent) = zip_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }

    let mut out = std::fs::File::create(zip_path)
        .map_err(|e| format!("Failed to create {}: {e}", zip_path.display()))?;
    let mut entries = Vec::new();

    for file in files {
        let bytes =
            std::fs::read(&file).map_err(|e| format!("Failed to read {}: {e}", file.display()))?;
        let rel = file
            .strip_prefix(source_dir)
            .map_err(|e| format!("Failed to build zip path for {}: {e}", file.display()))?;
        let name = zip_name(prefix, rel);
        write_file_entry(&mut out, &mut entries, &name, &bytes)?;
    }

    if let Some(version) = patch_version.filter(|v| !v.trim().is_empty()) {
        write_file_entry(
            &mut out,
            &mut entries,
            "patch_version.txt",
            version.trim().as_bytes(),
        )?;
    }

    let central_offset = out
        .stream_position()
        .map_err(|e| format!("Failed to seek zip: {e}"))? as u32;
    for entry in &entries {
        write_u32(&mut out, 0x0201_4b50)?;
        write_u16(&mut out, 20)?;
        write_u16(&mut out, 20)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, ZIP_METHOD_DEFLATE)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u32(&mut out, entry.crc32)?;
        write_u32(&mut out, entry.compressed_size)?;
        write_u32(&mut out, entry.uncompressed_size)?;
        write_u16(&mut out, entry.name.len() as u16)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u32(&mut out, 0)?;
        write_u32(&mut out, entry.offset)?;
        out.write_all(entry.name.as_bytes())
            .map_err(|e| format!("Failed to write zip: {e}"))?;
    }
    let central_size = out
        .stream_position()
        .map_err(|e| format!("Failed to seek zip: {e}"))? as u32
        - central_offset;

    write_u32(&mut out, 0x0605_4b50)?;
    write_u16(&mut out, 0)?;
    write_u16(&mut out, 0)?;
    write_u16(&mut out, entries.len() as u16)?;
    write_u16(&mut out, entries.len() as u16)?;
    write_u32(&mut out, central_size)?;
    write_u32(&mut out, central_offset)?;
    write_u16(&mut out, 0)?;

    Ok(())
}

const ZIP_METHOD_DEFLATE: u16 = 8;

fn write_file_entry(
    out: &mut std::fs::File,
    entries: &mut Vec<ZipEntry>,
    name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    if bytes.len() > u32::MAX as usize {
        return Err(format!("{} is too large for this zip writer.", name));
    }
    let offset = out
        .stream_position()
        .map_err(|e| format!("Failed to seek zip: {e}"))? as u32;
    let crc32 = crc32(bytes);
    let uncompressed_size = bytes.len() as u32;
    let compressed = deflate_max(bytes)?;
    if compressed.len() > u32::MAX as usize {
        return Err(format!("{} is too large for this zip writer.", name));
    }
    let compressed_size = compressed.len() as u32;

    write_u32(out, 0x0403_4b50)?;
    write_u16(out, 20)?;
    write_u16(out, 0)?;
    write_u16(out, ZIP_METHOD_DEFLATE)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u32(out, crc32)?;
    write_u32(out, compressed_size)?;
    write_u32(out, uncompressed_size)?;
    write_u16(out, name.len() as u16)?;
    write_u16(out, 0)?;
    out.write_all(name.as_bytes())
        .map_err(|e| format!("Failed to write zip: {e}"))?;
    out.write_all(&compressed)
        .map_err(|e| format!("Failed to write zip: {e}"))?;

    entries.push(ZipEntry {
        name: name.to_string(),
        crc32,
        compressed_size,
        uncompressed_size,
        offset,
    });
    Ok(())
}

fn deflate_max(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(bytes)
        .map_err(|e| format!("Failed to compress zip entry: {e}"))?;
    encoder
        .finish()
        .map_err(|e| format!("Failed to finish zip entry compression: {e}"))
}

fn collect_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_files_recursive(dir, &mut files)?;
    Ok(files)
}

fn collect_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in
        std::fs::read_dir(dir).map_err(|e| format!("Failed to read {}: {e}", dir.display()))?
    {
        let path = entry
            .map_err(|e| format!("Failed to read directory entry in {}: {e}", dir.display()))?
            .path();
        if is_internal_work_path(&path) {
            continue;
        }
        if path.is_dir() {
            collect_files_recursive(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn is_internal_work_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("_nier_"))
}

fn zip_name(prefix: Option<&str>, rel: &Path) -> String {
    let rel = rel
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    match prefix {
        Some(prefix) if !prefix.trim_matches('/').is_empty() => {
            format!("{}/{}", prefix.trim_matches('/'), rel)
        }
        _ => rel,
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn write_u16(out: &mut std::fs::File, value: u16) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| format!("Failed to write zip: {e}"))
}

fn write_u32(out: &mut std::fs::File, value: u32) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| format!("Failed to write zip: {e}"))
}

/// Validate the completed Switch ZIP, including its packaged CPK and IPS.
pub fn verify_switch_zip(zip: &Path) -> Result<(), String> {
    use std::collections::BTreeMap;
    use std::io::Read;

    let bytes = std::fs::read(zip)
        .map_err(|e| format!("Cannot read Switch ZIP {}: {e}", zip.display()))?;
    let read_u16 = |pos: usize| -> Result<usize, String> {
        let slice = bytes.get(pos..pos + 2).ok_or("Truncated ZIP u16")?;
        Ok(u16::from_le_bytes([slice[0], slice[1]]) as usize)
    };
    let read_u32 = |pos: usize| -> Result<usize, String> {
        let slice = bytes.get(pos..pos + 4).ok_or("Truncated ZIP u32")?;
        Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as usize)
    };
    let start = bytes.len().saturating_sub(65557);
    let eocd = (start..bytes.len().saturating_sub(3)).rev()
        .find(|&i| bytes.get(i..i + 4) == Some(&b"PK\x05\x06"[..]))
        .ok_or("Missing ZIP end-of-central-directory")?;
    let count = read_u16(eocd + 10)?;
    let directory_size = read_u32(eocd + 12)?;
    let directory_offset = read_u32(eocd + 16)?;
    let directory_end = directory_offset.checked_add(directory_size)
        .ok_or("ZIP central directory offset overflow")?;
    if directory_end > eocd { return Err("Invalid central directory bounds".into()); }
    let mut pos = directory_offset;
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        if bytes.get(pos..pos + 4) != Some(&b"PK\x01\x02"[..]) {
            return Err("Invalid ZIP central entry signature".into());
        }
        let size = read_u32(pos + 20)?;
        let name_len = read_u16(pos + 28)?;
        let extra_len = read_u16(pos + 30)?;
        let comment_len = read_u16(pos + 32)?;
        let local_offset = read_u32(pos + 42)?;
        let name_bytes = bytes.get(pos + 46..pos + 46 + name_len)
            .ok_or("Truncated ZIP entry name")?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|e| format!("Invalid ZIP entry name: {e}"))?.to_owned();
        if entries.insert(name.clone(), (size, local_offset)).is_some() {
            return Err(format!("Duplicate ZIP file {name}"));
        }
        pos = pos.checked_add(46 + name_len + extra_len + comment_len)
            .ok_or("ZIP directory overflow")?;
    }
    if pos != directory_end { return Err("ZIP directory size mismatch".into()); }
    let root = "Nier Automata Unburger/";
    let cpk_name = format!("{root}romfs/data300.cpk");
    let ips_name = format!("{root}exefs/{}.ips", crate::switch_subtitle_cpk::UPDATE_102_BUILD_ID);
    if entries.keys().any(|name| name.starts_with(&format!("{root}romfs/subtitle/"))) {
        return Err("Legacy loose subtitle/ files present in Switch ZIP".into());
    }
    if entries.keys().any(|name| name.starts_with("NierUnburger/") || name.starts_with("Nier Automata Unburger/romfs/romfs/")) {
        return Err("Incorrect Switch ZIP folder nesting".into());
    }
    let decode_entry = |name: &str| -> Result<Vec<u8>, String> {
        let (compressed_size, offset) = entries.get(name)
            .ok_or_else(|| format!("Switch ZIP missing {name}"))?;
        if bytes.get(*offset..*offset + 4) != Some(&b"PK\x03\x04"[..]) {
            return Err(format!("Corrupt ZIP local header: {name}"));
        }
        let method = read_u16(*offset + 8)?;
        if method != 8 { return Err(format!("Unsupported compression method for {name}")); }
        let local_name_len = read_u16(*offset + 26)?;
        let local_extra_len = read_u16(*offset + 28)?;
        let from = (*offset).checked_add(30 + local_name_len + local_extra_len)
            .ok_or("ZIP local header overflow")?;
        let compressed = bytes.get(from..from + *compressed_size)
            .ok_or("Truncated ZIP payload")?;
        let mut decoder = flate2::read::DeflateDecoder::new(compressed);
        let mut result = Vec::new();
        decoder.read_to_end(&mut result).map_err(|e| format!("Cannot decompress {name}: {e}"))?;
        Ok(result)
    };
    let ips = decode_entry(&ips_name)?;
    if ips != crate::switch_subtitle_cpk::UPDATE_102_IPS {
        return Err("Switch ZIP IPS does not match the supported game update".into());
    }
    let cpk = decode_entry(&cpk_name)?;
    if !cpk.starts_with(b"CPK ") || cpk.len() < 2048 {
        return Err("Invalid/empty data300.cpk in Switch ZIP".into());
    }
    Ok(())
}
