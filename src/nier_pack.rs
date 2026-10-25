use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const BUILD_LOCALIZATION: &str = include_str!("../nier-pack/build_localization.sh");

#[derive(Clone, Debug)]
pub struct NierPackOptions {
    pub resources_dir: PathBuf,
    pub translations_dir: PathBuf,
    pub output_dir: PathBuf,
    pub include_ui_mcd: bool,
}

#[derive(Clone, Debug, Default)]
pub struct NierPackReport {
    pub jobs_total: usize,
    pub jobs_built: usize,
    pub jobs_skipped_mcd: usize,
    pub archives_written: usize,
    pub ui_texture_archives_written: usize,
    pub missing_translations: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct NierFontReport {
    pub fonts_built: usize,
}

#[derive(Clone, Debug, Default)]
pub struct NierSwitchUiReport {
    pub switch_data_dir: Option<PathBuf>,
    pub jobs_total: usize,
    pub jobs_built: usize,
    pub dat_archives_written: usize,
    pub dtt_archives_copied: usize,
    pub missing_translations: Vec<String>,
    pub missing_sources: Vec<String>,
}

#[derive(Clone, Debug)]
struct BuildJob {
    dat_file: String,
    name: String,
    ext: String,
}

pub fn build_non_ui(options: &NierPackOptions) -> Result<NierPackReport, String> {
    let jobs = parse_build_jobs();
    let mut report = NierPackReport {
        jobs_total: jobs.len(),
        ..Default::default()
    };
    let mut archives: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    let fonts_config_path = options.resources_dir.join("fonts").join("fonts.json");
    let mut mcd_fonts: Option<HashMap<u32, McdRuntimeFont>> = None;

    for job in jobs {
        if job.ext == "mcd" && !options.include_ui_mcd {
            report.jobs_skipped_mcd += 1;
            continue;
        }

        match job.ext.as_str() {
            "bin" | "pak" | "smd" | "tmd" => {
                let properties = options
                    .translations_dir
                    .join(format!("{}.properties", job.name));
                if !properties.is_file() {
                    report
                        .missing_translations
                        .push(properties.display().to_string());
                    continue;
                }
                let input = options
                    .resources_dir
                    .join("unpacked")
                    .join(&job.dat_file)
                    .join(format!("{}.{}", job.name, job.ext));
                let bytes = std::fs::read(&input)
                    .map_err(|e| format!("Failed to read {}: {e}", input.display()))?;
                let mapping = parse_properties_file(&properties)?;
                let patched = match job.ext.as_str() {
                    "bin" => put_bin_strings(&bytes, &mapping)?,
                    "pak" => put_pak_strings(&bytes, &mapping)?,
                    "smd" => put_smd_strings(&bytes, &mapping)?,
                    "tmd" => put_tmd_strings(&bytes, &mapping)?,
                    _ => unreachable!(),
                };
                archives
                    .entry(job.dat_file.clone())
                    .or_default()
                    .push((format!("{}.{}", job.name, job.ext), patched));
                report.jobs_built += 1;
            }
            "mcd" => {
                let properties = options
                    .translations_dir
                    .join(format!("{}.properties", job.name));
                if !properties.is_file() {
                    report
                        .missing_translations
                        .push(properties.display().to_string());
                    continue;
                }
                if mcd_fonts.is_none() {
                    mcd_fonts = Some(load_mcd_fonts(&fonts_config_path, &options.resources_dir)?);
                }
                let fonts = mcd_fonts.as_ref().expect("MCD fonts were loaded");
                let input = options
                    .resources_dir
                    .join("unpacked")
                    .join(&job.dat_file)
                    .join(format!("{}.{}", job.name, job.ext));
                let bytes = std::fs::read(&input)
                    .map_err(|e| format!("Failed to read {}: {e}", input.display()))?;
                let mapping = parse_properties_file(&properties)?;
                let work_dir = options
                    .output_dir
                    .join("_nier_mcd_work")
                    .join(safe_work_name(&job.dat_file))
                    .join(&job.name);
                let (patched_mcd, patched_wta, patched_wtp) =
                    put_mcd_strings_with_texture(options, &job, &bytes, &mapping, fonts, &work_dir)?;
                archives
                    .entry(job.dat_file.clone())
                    .or_default()
                    .push((format!("{}.mcd", job.name), patched_mcd));
                archives
                    .entry(job.dat_file.clone())
                    .or_default()
                    .push((format!("{}.wta", job.name), patched_wta));
                archives
                    .entry(mcd_dtt_file(&job.dat_file)?)
                    .or_default()
                    .push((format!("{}.wtp", job.name), patched_wtp));
                report.jobs_built += 1;
            }
            "txt" => {
                let source = options
                    .translations_dir
                    .join(format!("{}.txt", expand_name(&job.name)));
                if !source.is_file() {
                    report
                        .missing_translations
                        .push(source.display().to_string());
                    continue;
                }
                let target = options
                    .output_dir
                    .join("novel")
                    .join(source.file_name().unwrap_or_default());
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
                }
                std::fs::copy(&source, &target).map_err(|e| {
                    format!(
                        "Failed to copy {} to {}: {e}",
                        source.display(),
                        target.display()
                    )
                })?;
                report.jobs_built += 1;
            }
            _ => {}
        }
    }

    for (dat_file, replacements) in archives {
        let source = options.resources_dir.join("data").join(&dat_file);
        let target = options.output_dir.join(&dat_file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        repack_dat(&source, &target, &replacements)?;
        if dat_file.ends_with(".dtt") && dat_file.starts_with("ui/") {
            report.ui_texture_archives_written += 1;
        }
        report.archives_written += 1;
    }

    Ok(report)
}

const POLISH_GLYPHS: &[(char, &str)] = &[
    ('\u{0104}', "0104"),
    ('\u{0105}', "0105"),
    ('\u{0106}', "0106"),
    ('\u{0107}', "0107"),
    ('\u{0118}', "0118"),
    ('\u{0119}', "0119"),
    ('\u{0141}', "0141"),
    ('\u{0142}', "0142"),
    ('\u{0143}', "0143"),
    ('\u{0144}', "0144"),
    ('\u{015a}', "015a"),
    ('\u{015b}', "015b"),
    ('\u{0179}', "0179"),
    ('\u{017a}', "017a"),
    ('\u{017b}', "017b"),
    ('\u{017c}', "017c"),
];

const KERNING_CLONES: &[(char, char)] = &[
    ('A', '\u{0104}'),
    ('a', '\u{0105}'),
    ('C', '\u{0106}'),
    ('c', '\u{0107}'),
    ('E', '\u{0118}'),
    ('e', '\u{0119}'),
    ('N', '\u{0143}'),
    ('n', '\u{0144}'),
    ('S', '\u{015a}'),
    ('s', '\u{015b}'),
    ('Z', '\u{0179}'),
    ('z', '\u{017a}'),
    ('Z', '\u{017b}'),
    ('z', '\u{017c}'),
];

pub fn build_fonts(
    options: &NierPackOptions,
    magick_command: &str,
) -> Result<NierFontReport, String> {
    let mut report = NierFontReport::default();
    for font_id in ["00", "01", "04", "05", "11"] {
        build_font(options, magick_command, font_id)?;
        report.fonts_built += 1;
    }
    Ok(report)
}

pub fn build_switch_ui_text(options: &NierPackOptions) -> Result<NierSwitchUiReport, String> {
    let mut report = NierSwitchUiReport::default();
    let Some(switch_data_dir) = find_switch_ui_data_dir(&options.resources_dir) else {
        return Ok(report);
    };
    report.switch_data_dir = Some(switch_data_dir.clone());

    for job in parse_build_jobs()
        .into_iter()
        .filter(|job| job.ext == "mcd" && job.dat_file.starts_with("ui/"))
    {
        report.jobs_total += 1;
        let properties = options
            .translations_dir
            .join(format!("{}.properties", job.name));
        if !properties.is_file() {
            report
                .missing_translations
                .push(properties.display().to_string());
            continue;
        }

        let source_dat = switch_data_dir.join(&job.dat_file);
        if !source_dat.is_file() {
            report.missing_sources.push(source_dat.display().to_string());
            continue;
        }

        let member_name = format!("{}.mcd", job.name);
        let mcd_bytes = match read_dat_member(&source_dat, &member_name) {
            Ok(bytes) => bytes,
            Err(err) => {
                report.missing_sources.push(err);
                continue;
            }
        };
        let mapping = parse_properties_file(&properties)?;
        let mut mcd = McdFile::parse(&mcd_bytes)?;
        mcd.put_strings_with_existing_glyphs(&mapping)?;
        let patched_mcd = mcd.serialize()?;

        let target_dat = options.output_dir.join(&job.dat_file);
        if let Some(parent) = target_dat.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        repack_dat(&source_dat, &target_dat, &[(member_name, patched_mcd)])?;
        report.jobs_built += 1;
        report.dat_archives_written += 1;

        let dtt_file = mcd_dtt_file(&job.dat_file)?;
        let source_dtt = switch_data_dir.join(&dtt_file);
        if source_dtt.is_file() {
            let target_dtt = options.output_dir.join(&dtt_file);
            if let Some(parent) = target_dtt.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
            }
            std::fs::copy(&source_dtt, &target_dtt).map_err(|e| {
                format!(
                    "Failed to copy Switch UI texture archive {} to {}: {e}",
                    source_dtt.display(),
                    target_dtt.display()
                )
            })?;
            report.dtt_archives_copied += 1;
        } else {
            report.missing_sources.push(source_dtt.display().to_string());
        }
    }

    Ok(report)
}

fn build_font(
    options: &NierPackOptions,
    magick_command: &str,
    font_id: &str,
) -> Result<(), String> {
    let font_dat_dir = options
        .resources_dir
        .join("unpacked")
        .join("font")
        .join(format!("font_{font_id}.dat"));
    let font_dtt_dir = options
        .resources_dir
        .join("unpacked")
        .join("font")
        .join(format!("font_{font_id}.dtt"));
    let glyph_dir = options.resources_dir.join("fonts").join(font_id);

    let mut missing = Vec::new();
    for (_, hex) in POLISH_GLYPHS {
        let path = glyph_dir.join(format!("{hex}.png"));
        if !path.is_file() {
            missing.push(path.display().to_string());
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "Missing Nier custom font glyph PNGs for font {font_id}:\n{}",
            missing.join("\n")
        ));
    }

    let ftb_path = font_dat_dir.join(format!("font_{font_id}.ftb"));
    let wta_path = font_dat_dir.join(format!("font_{font_id}.wta"));
    let wtp_path = font_dtt_dir.join(format!("font_{font_id}.wtp"));
    let page = last_font_texture_page(&font_dtt_dir)?;
    let dds_path = font_dtt_dir.join(format!("font_{font_id}.wtp_{page:03}.dds"));

    let mut ftb = FtbFile::parse(
        &std::fs::read(&ftb_path)
            .map_err(|e| format!("Failed to read {}: {e}", ftb_path.display()))?,
    )?;

    let temp_dir = options
        .output_dir
        .join("_nier_font_work")
        .join(format!("font_{font_id}"));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create {}: {e}", temp_dir.display()))?;

    let texture_png = temp_dir.join("texture.png");
    run_magick(
        magick_command,
        &[dds_path.as_path(), texture_png.as_path()],
        "convert font DDS to PNG",
    )?;
    let mut texture = image::open(&texture_png)
        .map_err(|e| format!("Failed to open {}: {e}", texture_png.display()))?
        .to_rgba8();
    let texture_size = (texture.width(), texture.height());

    for (ch, hex) in POLISH_GLYPHS {
        let glyph_path = glyph_dir.join(format!("{hex}.png"));
        let glyph = image::open(&glyph_path)
            .map_err(|e| format!("Failed to open {}: {e}", glyph_path.display()))?
            .to_rgba8();
        let (x, y) =
            ftb.find_space_for_glyph((glyph.width(), glyph.height()), texture_size, page)?;
        overlay_rgba(&mut texture, &glyph, x, y);
        ftb.add_character(*ch, page, glyph.width(), glyph.height(), x, y);
    }

    texture
        .save(&texture_png)
        .map_err(|e| format!("Failed to write {}: {e}", texture_png.display()))?;
    let converted_dds = temp_dir.join("texture.dds");
    run_magick(
        magick_command,
        &[
            Path::new("-define"),
            Path::new("dds:mipmaps=0"),
            texture_png.as_path(),
            converted_dds.as_path(),
        ],
        "convert patched font PNG to DDS",
    )?;

    let mut dat_replacements = Vec::new();
    dat_replacements.push((format!("font_{font_id}.ftb"), ftb.serialize()?));

    let ktb_path = font_dat_dir.join(format!("font_{font_id}.ktb"));
    if ktb_path.is_file() {
        let mut ktb = KtbFile::parse(
            &std::fs::read(&ktb_path)
                .map_err(|e| format!("Failed to read {}: {e}", ktb_path.display()))?,
        )?;
        for (from, to) in KERNING_CLONES {
            ktb.clone_kerning(*from, *to);
        }
        dat_replacements.push((format!("font_{font_id}.ktb"), ktb.serialize()?));
    }

    let mut wta = WtaFile::parse(
        &std::fs::read(&wta_path)
            .map_err(|e| format!("Failed to read {}: {e}", wta_path.display()))?,
    )?;
    let wtp_bytes = repack_wtp(
        &mut wta,
        &std::fs::read(&wtp_path)
            .map_err(|e| format!("Failed to read {}: {e}", wtp_path.display()))?,
        &[(
            page as usize,
            std::fs::read(&converted_dds)
                .map_err(|e| format!("Failed to read {}: {e}", converted_dds.display()))?,
        )],
    )?;
    dat_replacements.push((format!("font_{font_id}.wta"), wta.serialize()));

    let source_dat = options
        .resources_dir
        .join("data")
        .join("font")
        .join(format!("font_{font_id}.dat"));
    let target_dat = options
        .output_dir
        .join("font")
        .join(format!("font_{font_id}.dat"));
    if let Some(parent) = target_dat.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    repack_dat(&source_dat, &target_dat, &dat_replacements)?;

    let source_dtt = options
        .resources_dir
        .join("data")
        .join("font")
        .join(format!("font_{font_id}.dtt"));
    let target_dtt = options
        .output_dir
        .join("font")
        .join(format!("font_{font_id}.dtt"));
    repack_dat(
        &source_dtt,
        &target_dtt,
        &[(format!("font_{font_id}.wtp"), wtp_bytes)],
    )?;
    Ok(())
}

fn parse_build_jobs() -> Vec<BuildJob> {
    BUILD_LOCALIZATION
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("put_strings ") {
                return None;
            }
            let first = line.split(';').next().unwrap_or(line);
            let parts: Vec<_> = first.split_whitespace().collect();
            if parts.len() < 4 {
                return None;
            }
            Some(BuildJob {
                dat_file: expand_name(parts[1]),
                name: expand_name(parts[2]),
                ext: parts[3].to_string(),
            })
        })
        .collect()
}

fn expand_name(value: &str) -> String {
    value.replace("${LANG2}", "_us").replace("${LANG3}", "_eng")
}

fn parse_properties_file(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let mut map = BTreeMap::new();
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        map.insert(decode_properties(key)?, decode_properties(value)?);
    }
    Ok(map)
}

fn decode_properties(value: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                let code = u16::from_str_radix(&hex, 16)
                    .map_err(|e| format!("Invalid unicode escape \\u{hex}: {e}"))?;
                out.push(char::from_u32(code as u32).ok_or_else(|| {
                    format!("Invalid unicode scalar in properties escape: \\u{hex}")
                })?);
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    Ok(out)
}

#[derive(Clone)]
struct DatEntry {
    name: String,
    bytes: Vec<u8>,
}

struct DatFile {
    magic: [u8; 4],
    unknown: [u8; 4],
    crc_table: Vec<u8>,
    files: Vec<DatEntry>,
}

#[derive(Clone)]
struct FtbChar {
    ch: char,
    texture_id: u16,
    width: u16,
    height: u16,
    u: u16,
    v: u16,
}

struct FtbFile {
    magic: Vec<u8>,
    textures_count: u16,
    unknown: [u8; 2],
    textures: Vec<u8>,
    chars: Vec<FtbChar>,
}

impl FtbFile {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor::new(bytes);
        let magic = read_vec(&mut c, 118)?;
        if !magic.starts_with(b"FTB ") {
            return Err("Invalid FTB magic".into());
        }
        let textures_count = read_u16_le(&mut c)?;
        let unknown = read_array::<2, _>(&mut c)?;
        let chars_count = read_u16_le(&mut c)? as usize;
        let textures_offset = read_u32_le(&mut c)? as u64;
        let chars_offset = read_u32_le(&mut c)? as u64;
        let _chars_offset2 = read_u32_le(&mut c)?;

        c.set_position(textures_offset);
        let textures = read_vec(&mut c, textures_count as usize * 16)?;

        c.set_position(chars_offset);
        let mut chars = Vec::with_capacity(chars_count);
        for _ in 0..chars_count {
            let ch = read_char_u16(&mut c)?;
            chars.push(FtbChar {
                ch,
                texture_id: read_u16_le(&mut c)?,
                width: read_u16_le(&mut c)?,
                height: read_u16_le(&mut c)?,
                u: read_u16_le(&mut c)?,
                v: read_u16_le(&mut c)?,
            });
        }

        Ok(Self {
            magic,
            textures_count,
            unknown,
            textures,
            chars,
        })
    }

    fn find_space_for_glyph(
        &self,
        glyph_size: (u32, u32),
        texture_size: (u32, u32),
        page: u16,
    ) -> Result<(u32, u32), String> {
        let w = glyph_size.0;
        let h = glyph_size.1;
        let mut glyph_x = 0;
        let mut glyph_y = texture_size.1;
        let max_x = texture_size.0.saturating_sub(w + 1);
        for x in (0..max_x).step_by(32) {
            let mut y = 0;
            for c in self.chars.iter().filter(|c| c.texture_id == page) {
                let cu = c.u as u32;
                let cv = c.v as u32;
                let cw = c.width as u32;
                if x < cu + cw && x + w > cu {
                    y = y.max(cv + c.height as u32);
                }
            }
            if y < glyph_y {
                glyph_x = x;
                glyph_y = y;
            }
        }
        if glyph_y <= texture_size.1.saturating_sub(h + 1) {
            Ok((glyph_x, glyph_y))
        } else {
            Err(format!("Cannot place glyph on font texture page {page}"))
        }
    }

    fn add_character(&mut self, ch: char, texture: u16, width: u32, height: u32, u: u32, v: u32) {
        self.chars.push(FtbChar {
            ch,
            texture_id: texture,
            width: width.saturating_sub(1) as u16,
            height: height.saturating_sub(1) as u16,
            u: u as u16,
            v: v as u16,
        });
        self.chars.sort_by_key(|c| c.ch);
    }

    fn serialize(&self) -> Result<Vec<u8>, String> {
        let textures_offset = 118 + 2 + 2 + 2 + 4 + 4 + 4;
        let chars_offset = textures_offset + self.textures.len();
        let mut out = Vec::new();
        out.extend(&self.magic);
        write_u16_le(&mut out, self.textures_count);
        out.extend(self.unknown);
        write_u16_le(&mut out, self.chars.len() as u16);
        write_u32_le(&mut out, textures_offset as u32);
        write_u32_le(&mut out, chars_offset as u32);
        write_u32_le(&mut out, chars_offset as u32);
        out.extend(&self.textures);
        for c in &self.chars {
            write_char_u16(&mut out, c.ch)?;
            write_u16_le(&mut out, c.texture_id);
            write_u16_le(&mut out, c.width);
            write_u16_le(&mut out, c.height);
            write_u16_le(&mut out, c.u);
            write_u16_le(&mut out, c.v);
        }
        out.extend(vec![0; 0x88]);
        Ok(out)
    }
}

struct KtbFile {
    entries: BTreeMap<(char, char), u16>,
}

impl KtbFile {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor::new(bytes);
        let count = read_u16_le(&mut c)? as usize;
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let left = read_char_u16(&mut c)?;
            let right = read_char_u16(&mut c)?;
            let kerning = read_u16_le(&mut c)?;
            entries.insert((left, right), kerning);
        }
        Ok(Self { entries })
    }

    fn get_kerning(&self, left: char, right: char) -> u16 {
        self.entries.get(&(left, right)).copied().unwrap_or(0)
    }

    fn clone_kerning(&mut self, from: char, to: char) {
        let mut new_entries = BTreeMap::new();
        for (&(left, right), &kerning) in &self.entries {
            if left == from || right == from {
                let new_left = if left == from { to } else { left };
                let new_right = if right == from { to } else { right };
                new_entries.insert((new_left, new_right), kerning);
            }
        }
        self.entries.extend(new_entries);
    }

    fn serialize(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        write_u16_le(&mut out, self.entries.len() as u16);
        for ((left, right), kerning) in &self.entries {
            write_char_u16(&mut out, *left)?;
            write_char_u16(&mut out, *right)?;
            write_u16_le(&mut out, *kerning);
        }
        Ok(out)
    }
}

struct WtaEntry {
    offset: u32,
    size: u32,
    flags: [u8; 4],
    idx: [u8; 4],
    info: Option<Vec<u8>>,
}

struct WtaFile {
    magic: [u8; 8],
    textures: Vec<WtaEntry>,
    has_info_table: bool,
}

impl WtaFile {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor::new(bytes);
        let magic = read_array::<8, _>(&mut c)?;
        if !magic.starts_with(b"WTB\0") {
            return Err("Invalid WTA magic".into());
        }
        let textures_count = read_u32_le(&mut c)? as usize;
        let offsets_offset = read_u32_le(&mut c)? as u64;
        let size_offset = read_u32_le(&mut c)? as u64;
        let flags_offset = read_u32_le(&mut c)? as u64;
        let idx_offset = read_u32_le(&mut c)? as u64;
        let info_offset = read_u32_le(&mut c)? as usize;

        let mut textures = (0..textures_count)
            .map(|_| WtaEntry {
                offset: 0,
                size: 0,
                flags: [0; 4],
                idx: [0; 4],
                info: None,
            })
            .collect::<Vec<_>>();

        c.set_position(offsets_offset);
        for entry in &mut textures {
            entry.offset = read_u32_le(&mut c)?;
        }
        c.set_position(size_offset);
        for entry in &mut textures {
            entry.size = read_u32_le(&mut c)?;
        }
        c.set_position(flags_offset);
        for entry in &mut textures {
            entry.flags = read_array::<4, _>(&mut c)?;
        }
        c.set_position(idx_offset);
        for entry in &mut textures {
            entry.idx = read_array::<4, _>(&mut c)?;
        }

        let has_info_table = info_offset > 0;
        if has_info_table {
            let mut info_record_size = (bytes.len() - info_offset) / textures_count;
            if info_record_size < 256 && info_record_size > 0 {
                info_record_size = 20;
            }
            c.set_position(info_offset as u64);
            for entry in &mut textures {
                entry.info = Some(read_vec(&mut c, info_record_size)?);
            }
        }

        Ok(Self {
            magic,
            textures,
            has_info_table,
        })
    }

    fn serialize(&self) -> Vec<u8> {
        let mut out = vec![0; 32];
        let offsets_offset = out.len();
        let mut texture_offset = 0u32;
        for texture in &self.textures {
            write_u32_le(&mut out, texture_offset);
            texture_offset += texture.size;
            texture_offset += padding_len(texture_offset as usize, 0x1000) as u32;
        }
        pad(&mut out, 32);

        let size_offset = out.len();
        for texture in &self.textures {
            write_u32_le(&mut out, texture.size);
        }
        pad(&mut out, 32);

        let flags_offset = out.len();
        for texture in &self.textures {
            out.extend(texture.flags);
        }
        pad(&mut out, 32);

        let idx_offset = out.len();
        for texture in &self.textures {
            out.extend(texture.idx);
        }
        if self.has_info_table {
            pad(&mut out, 32);
        } else {
            pad(&mut out, 16);
        }

        let info_offset = if self.has_info_table {
            let offset = out.len();
            for texture in &self.textures {
                if let Some(info) = &texture.info {
                    out.extend(info);
                }
            }
            pad(&mut out, 16);
            offset
        } else {
            0
        };

        let mut header = Vec::new();
        header.extend(self.magic);
        write_u32_le(&mut header, self.textures.len() as u32);
        write_u32_le(&mut header, offsets_offset as u32);
        write_u32_le(&mut header, size_offset as u32);
        write_u32_le(&mut header, flags_offset as u32);
        write_u32_le(&mut header, idx_offset as u32);
        write_u32_le(&mut header, info_offset as u32);
        out[0..32].copy_from_slice(&header);
        out
    }
}

#[derive(Clone)]
struct McdSymbol {
    font_id: u16,
    ch: char,
    glyph_id: u32,
}

#[derive(Clone)]
struct McdGlyph {
    texture: u32,
    u1: f32,
    v1: f32,
    u2: f32,
    v2: f32,
    width: f32,
    height: f32,
    above: f32,
    below: f32,
    horiz: f32,
}

#[derive(Clone)]
struct McdFont {
    id: u32,
    width: f32,
    height: f32,
    below: f32,
    horiz: f32,
}

#[derive(Clone)]
struct McdEvent {
    id: u32,
    idx: u32,
    name: String,
}

#[derive(Clone)]
struct McdLine {
    content: Vec<u16>,
    padding: u32,
    below: f32,
    horiz: f32,
}

#[derive(Clone)]
struct McdText {
    lines: Vec<McdLine>,
    vpos: u32,
    hpos: u32,
    font: u32,
}

#[derive(Clone)]
struct McdMessage {
    texts: Vec<McdText>,
    seq_number: u32,
    event_id: u32,
}

struct McdFile {
    messages: Vec<McdMessage>,
    symbols: Vec<McdSymbol>,
    glyphs: Vec<McdGlyph>,
    fonts: Vec<McdFont>,
    events: Vec<McdEvent>,
}

impl McdFile {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor::new(bytes);
        let messages_offset = read_u32_le(&mut c)? as u64;
        let messages_count = read_u32_le(&mut c)? as usize;
        let symbols_offset = read_u32_le(&mut c)? as u64;
        let symbols_count = read_u32_le(&mut c)? as usize;
        let glyphs_offset = read_u32_le(&mut c)? as u64;
        let glyphs_count = read_u32_le(&mut c)? as usize;
        let fonts_offset = read_u32_le(&mut c)? as u64;
        let fonts_count = read_u32_le(&mut c)? as usize;
        let events_offset = read_u32_le(&mut c)? as u64;
        let events_count = read_u32_le(&mut c)? as usize;

        c.set_position(symbols_offset);
        let mut symbols = Vec::with_capacity(symbols_count);
        for _ in 0..symbols_count {
            symbols.push(McdSymbol {
                font_id: read_u16_le(&mut c)?,
                ch: read_char_u16(&mut c)?,
                glyph_id: read_u32_le(&mut c)?,
            });
        }

        c.set_position(glyphs_offset);
        let mut glyphs = Vec::with_capacity(glyphs_count);
        for _ in 0..glyphs_count {
            glyphs.push(McdGlyph {
                texture: read_u32_le(&mut c)?,
                u1: read_f32_le(&mut c)?,
                v1: read_f32_le(&mut c)?,
                u2: read_f32_le(&mut c)?,
                v2: read_f32_le(&mut c)?,
                width: read_f32_le(&mut c)?,
                height: read_f32_le(&mut c)?,
                above: read_f32_le(&mut c)?,
                below: read_f32_le(&mut c)?,
                horiz: read_f32_le(&mut c)?,
            });
        }

        c.set_position(fonts_offset);
        let mut fonts = Vec::with_capacity(fonts_count);
        for _ in 0..fonts_count {
            fonts.push(McdFont {
                id: read_u32_le(&mut c)?,
                width: read_f32_le(&mut c)?,
                height: read_f32_le(&mut c)?,
                below: read_f32_le(&mut c)?,
                horiz: read_f32_le(&mut c)?,
            });
        }

        c.set_position(events_offset);
        let mut events = Vec::with_capacity(events_count);
        for _ in 0..events_count {
            events.push(McdEvent {
                id: read_u32_le(&mut c)?,
                idx: read_u32_le(&mut c)?,
                name: read_fixed_utf8(&mut c, 32)?,
            });
        }

        c.set_position(messages_offset);
        let mut messages = Vec::with_capacity(messages_count);
        for _ in 0..messages_count {
            let texts_offset = read_u32_le(&mut c)? as u64;
            let texts_count = read_u32_le(&mut c)? as usize;
            let seq_number = read_u32_le(&mut c)?;
            let event_id = read_u32_le(&mut c)?;
            let last_pos = c.position();
            c.set_position(texts_offset);
            let mut texts = Vec::with_capacity(texts_count);
            for _ in 0..texts_count {
                let lines_offset = read_u32_le(&mut c)? as u64;
                let lines_count = read_u32_le(&mut c)? as usize;
                let vpos = read_u32_le(&mut c)?;
                let hpos = read_u32_le(&mut c)?;
                let font = read_u32_le(&mut c)?;
                let text_last_pos = c.position();
                c.set_position(lines_offset);
                let mut lines = Vec::with_capacity(lines_count);
                for _ in 0..lines_count {
                    let content_offset = read_u32_le(&mut c)? as u64;
                    let padding = read_u32_le(&mut c)?;
                    let content_length = read_u32_le(&mut c)? as usize;
                    let _content_length2 = read_u32_le(&mut c)?;
                    let below = read_f32_le(&mut c)?;
                    let horiz = read_f32_le(&mut c)?;
                    let line_last_pos = c.position();
                    c.set_position(content_offset);
                    let mut content = Vec::with_capacity(content_length);
                    for _ in 0..content_length {
                        content.push(read_mcd_content_u16(&mut c)?);
                    }
                    c.set_position(line_last_pos);
                    lines.push(McdLine {
                        content,
                        padding,
                        below,
                        horiz,
                    });
                }
                c.set_position(text_last_pos);
                texts.push(McdText {
                    lines,
                    vpos,
                    hpos,
                    font,
                });
            }
            c.set_position(last_pos);
            messages.push(McdMessage {
                texts,
                seq_number,
                event_id,
            });
        }

        Ok(Self {
            messages,
            symbols,
            glyphs,
            fonts,
            events,
        })
    }

    fn put_strings_with_fonts(
        &mut self,
        mapping: &BTreeMap<String, String>,
        fonts: &HashMap<u32, McdRuntimeFont>,
    ) -> Result<(), String> {
        let symbols = self.symbols.clone();
        let events_by_id: BTreeMap<u32, String> = self
            .events
            .iter()
            .map(|event| (event.id, event.name.clone()))
            .collect();
        for msg in &mut self.messages {
            let event = events_by_id
                .get(&msg.event_id)
                .ok_or_else(|| format!("MCD message event id {} not found", msg.event_id))?;
            let value = mapping
                .get(event)
                .ok_or_else(|| format!("Missing MCD mapping for {event}"))?;
            for text in &mut msg.texts {
                let font = fonts
                    .get(&text.font)
                    .ok_or_else(|| format!("MCD font {} is not present in fonts.json", text.font))?;
                text.lines = value
                    .split('\n')
                    .map(|line| McdLine::from_string_with_font(line, font, event, &symbols))
                    .collect::<Result<Vec<_>, _>>()?;
            }
        }
        Ok(())
    }

    fn put_strings_with_existing_glyphs(
        &mut self,
        mapping: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let mut symbols = self.symbols.clone();
        let fonts: BTreeMap<u32, McdFont> =
            self.fonts.iter().map(|font| (font.id, font.clone())).collect();
        let events_by_id: BTreeMap<u32, String> = self
            .events
            .iter()
            .map(|event| (event.id, event.name.clone()))
            .collect();
        for msg in &mut self.messages {
            let event = events_by_id
                .get(&msg.event_id)
                .ok_or_else(|| format!("MCD message event id {} not found", msg.event_id))?;
            let Some(value) = mapping.get(event) else {
                continue;
            };
            for text in &mut msg.texts {
                let font = fonts
                    .get(&text.font)
                    .ok_or_else(|| format!("MCD font {} is not present in source UI", text.font))?;
                let preferred_symbols = text.preferred_symbols(&symbols);
                text.lines = value
                    .split('\n')
                    .map(|line| {
                        McdLine::from_string_with_existing_glyphs(
                            line,
                            font,
                            event,
                            &mut symbols,
                            &preferred_symbols,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
        }
        self.symbols = symbols;
        Ok(())
    }

    fn fonts_for_messages(&self) -> Result<BTreeMap<String, HashSet<u32>>, String> {
        let events_by_id: BTreeMap<u32, String> = self
            .events
            .iter()
            .map(|event| (event.id, event.name.clone()))
            .collect();
        let mut result = BTreeMap::new();
        for msg in &self.messages {
            let event = events_by_id
                .get(&msg.event_id)
                .ok_or_else(|| format!("MCD message event id {} not found", msg.event_id))?;
            let fonts = result.entry(event.clone()).or_insert_with(HashSet::new);
            for text in &msg.texts {
                fonts.insert(text.font);
            }
        }
        Ok(result)
    }

    fn remove_all_glyphs(&mut self) {
        self.symbols.clear();
        self.glyphs.clear();
    }

    fn put_glyph(
        &mut self,
        ch: char,
        font_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        texture_width: u32,
        texture_height: u32,
    ) -> Result<(), String> {
        let font = self
            .fonts
            .iter()
            .find(|font| font.id == font_id)
            .ok_or_else(|| format!("MCD font {font_id} not found while generating glyph atlas"))?;
        self.glyphs.push(McdGlyph {
            texture: 0,
            u1: x as f32 / texture_width as f32,
            v1: y as f32 / texture_height as f32,
            u2: (x + width) as f32 / texture_width as f32,
            v2: (y + height) as f32 / texture_height as f32,
            width: width as f32,
            height: height as f32,
            above: 0.0,
            below: font.below,
            horiz: font.horiz,
        });
        self.symbols.push(McdSymbol {
            font_id: font_id as u16,
            ch,
            glyph_id: self.glyphs.len() as u32 - 1,
        });
        Ok(())
    }

    fn serialize(&self) -> Result<Vec<u8>, String> {
        let mut current_offset = 40usize;
        let mut strings = Vec::<Vec<u16>>::new();
        let mut strings_offsets = Vec::<usize>::new();
        let mut texts = Vec::<&McdText>::new();
        let mut texts_offsets = Vec::<usize>::new();
        let mut lines = Vec::<&McdLine>::new();
        let mut lines_offsets = Vec::<usize>::new();

        for message in &self.messages {
            for text in &message.texts {
                texts.push(text);
                for line in &text.lines {
                    strings_offsets.push(current_offset);
                    current_offset += line.content.len() * 2;
                    strings.push(line.content.clone());
                    lines.push(line);
                }
            }
        }
        current_offset += eager_padding_len(current_offset, 4);

        let messages_offset = current_offset;
        current_offset += self.messages.len() * 16;
        current_offset += eager_padding_len(current_offset, 4);

        for i in 0..texts.len() {
            texts_offsets.push(current_offset + i * 20);
        }
        current_offset += texts.len() * 20;
        current_offset += eager_padding_len(current_offset, 4);

        for i in 0..lines.len() {
            lines_offsets.push(current_offset + i * 24);
        }
        current_offset += lines.len() * 24;
        current_offset += eager_padding_len(current_offset, 4);

        let symbols_offset = current_offset;
        current_offset += self.symbols.len() * 8 + 4;
        let glyphs_offset = current_offset;
        current_offset += self.glyphs.len() * 40 + 4;
        let fonts_offset = current_offset;
        current_offset += self.fonts.len() * 20 + 4;
        let events_offset = current_offset;

        let mut out = Vec::new();
        write_u32_le(&mut out, messages_offset as u32);
        write_u32_le(&mut out, self.messages.len() as u32);
        write_u32_le(&mut out, symbols_offset as u32);
        write_u32_le(&mut out, self.symbols.len() as u32);
        write_u32_le(&mut out, glyphs_offset as u32);
        write_u32_le(&mut out, self.glyphs.len() as u32);
        write_u32_le(&mut out, fonts_offset as u32);
        write_u32_le(&mut out, self.fonts.len() as u32);
        write_u32_le(&mut out, events_offset as u32);
        write_u32_le(&mut out, self.events.len() as u32);

        for string in &strings {
            for value in string {
                write_u16_le(&mut out, *value);
            }
        }
        eager_pad(&mut out, 4);

        let mut texts_offset_idx = 0;
        for message in &self.messages {
            let texts_offset = texts_offsets[texts_offset_idx];
            texts_offset_idx += message.texts.len();
            write_u32_le(&mut out, texts_offset as u32);
            write_u32_le(&mut out, message.texts.len() as u32);
            write_u32_le(&mut out, message.seq_number);
            write_u32_le(&mut out, message.event_id);
        }
        eager_pad(&mut out, 4);

        let mut lines_offset_idx = 0;
        for text in &texts {
            let lines_offset = lines_offsets[lines_offset_idx];
            lines_offset_idx += text.lines.len();
            write_u32_le(&mut out, lines_offset as u32);
            write_u32_le(&mut out, text.lines.len() as u32);
            write_u32_le(&mut out, text.vpos);
            write_u32_le(&mut out, text.hpos);
            write_u32_le(&mut out, text.font);
        }
        eager_pad(&mut out, 4);

        for (idx, line) in lines.iter().enumerate() {
            write_u32_le(&mut out, strings_offsets[idx] as u32);
            write_u32_le(&mut out, line.padding);
            write_u32_le(&mut out, line.content.len() as u32);
            write_u32_le(&mut out, line.content.len() as u32);
            write_f32_le(&mut out, line.below);
            write_f32_le(&mut out, line.horiz);
        }
        eager_pad(&mut out, 4);

        for symbol in &self.symbols {
            write_u16_le(&mut out, symbol.font_id);
            write_char_u16(&mut out, symbol.ch)?;
            write_u32_le(&mut out, symbol.glyph_id);
        }
        eager_pad(&mut out, 4);

        for glyph in &self.glyphs {
            write_u32_le(&mut out, glyph.texture);
            write_f32_le(&mut out, glyph.u1);
            write_f32_le(&mut out, glyph.v1);
            write_f32_le(&mut out, glyph.u2);
            write_f32_le(&mut out, glyph.v2);
            write_f32_le(&mut out, glyph.width);
            write_f32_le(&mut out, glyph.height);
            write_f32_le(&mut out, glyph.above);
            write_f32_le(&mut out, glyph.below);
            write_f32_le(&mut out, glyph.horiz);
        }
        eager_pad(&mut out, 4);

        for font in &self.fonts {
            write_u32_le(&mut out, font.id);
            write_f32_le(&mut out, font.width);
            write_f32_le(&mut out, font.height);
            write_f32_le(&mut out, font.below);
            write_f32_le(&mut out, font.horiz);
        }
        eager_pad(&mut out, 4);

        for event in &self.events {
            write_u32_le(&mut out, event.id);
            write_u32_le(&mut out, event.idx);
            write_fixed_utf8(&mut out, &event.name, 32);
        }
        Ok(out)
    }
}

impl McdText {
    fn preferred_symbols(&self, symbols: &[McdSymbol]) -> HashMap<char, u16> {
        let mut preferred = HashMap::new();
        for line in &self.lines {
            line.append_preferred_symbols(symbols, &mut preferred);
        }
        preferred
    }
}

impl McdLine {
    fn append_preferred_symbols(
        &self,
        symbols: &[McdSymbol],
        preferred: &mut HashMap<char, u16>,
    ) {
        let mut idx = 0usize;
        while idx < self.content.len() {
            let value = self.content[idx];
            if value < 0x8000 {
                if let Some(symbol) = symbols.get(value as usize) {
                    preferred.entry(symbol.ch).or_insert(value);
                }
                idx += 2;
            } else if value == 0x8000 {
                idx += 1;
            } else {
                idx += 2;
            }
        }
    }

    fn from_string_with_font(
        value: &str,
        font: &McdRuntimeFont,
        event: &str,
        symbols: &[McdSymbol],
    ) -> Result<Self, String> {
        let mut content = Vec::new();
        let mut last_char = None;
        let mut iter = value.char_indices().peekable();
        while let Some((idx, ch)) = iter.next() {
            if value[idx..].starts_with("<special:") {
                let end = value[idx..]
                    .find('>')
                    .ok_or_else(|| format!("Unclosed MCD special tag in '{value}'"))?
                    + idx;
                let number_start = idx + "<special:".len();
                let number = value[number_start..end]
                    .parse::<u16>()
                    .map_err(|e| format!("Invalid MCD special tag in '{value}': {e}"))?;
                content.push(0x8020);
                content.push(number);
                while iter.peek().is_some_and(|(next_idx, _)| *next_idx <= end) {
                    iter.next();
                }
                last_char = None;
            } else if ch == ' ' {
                content.push(0x8001);
                content.push(font.id as u16);
                last_char = Some(ch);
            } else {
                let symbol_id = mcd_symbol_for_char_readonly(symbols, font.id, ch).ok_or_else(|| {
                    format!(
                        "No generated MCD glyph for '{}' in font {} while packing UI event {}",
                        ch, font.id, event
                    )
                })?;
                content.push(symbol_id);
                content.push(last_char.map(|left| font.kerning(left, ch)).unwrap_or(0));
                last_char = Some(ch);
            }
        }
        content.push(0x8000);
        Ok(Self {
            content,
            padding: 0,
            below: font.below,
            horiz: font.horiz,
        })
    }

    fn from_string_with_existing_glyphs(
        value: &str,
        font: &McdFont,
        event: &str,
        symbols: &mut Vec<McdSymbol>,
        preferred_symbols: &HashMap<char, u16>,
    ) -> Result<Self, String> {
        let mut content = Vec::new();
        let mut iter = value.char_indices().peekable();
        while let Some((idx, ch)) = iter.next() {
            if value[idx..].starts_with("<special:") {
                let end = value[idx..]
                    .find('>')
                    .ok_or_else(|| format!("Unclosed MCD special tag in '{value}'"))?
                    + idx;
                let number_start = idx + "<special:".len();
                let number = value[number_start..end]
                    .parse::<u16>()
                    .map_err(|e| format!("Invalid MCD special tag in '{value}': {e}"))?;
                content.push(0x8020);
                content.push(number);
                while iter.peek().is_some_and(|(next_idx, _)| *next_idx <= end) {
                    iter.next();
                }
            } else if ch == ' ' {
                content.push(0x8001);
                content.push(font.id as u16);
            } else {
                if let Some(symbol_id) =
                    switch_ui_symbol_for_char(symbols, font.id, ch, preferred_symbols)
                {
                    content.push(symbol_id);
                    content.push(0);
                } else {
                    content.push(0x8001);
                    content.push(font.id as u16);
                }
            }
        }
        content.push(0x8000);
        Ok(Self {
            content,
            padding: 0,
            below: font.below,
            horiz: font.horiz,
        })
    }
}

fn switch_ui_symbol_for_char(
    symbols: &[McdSymbol],
    font_id: u32,
    ch: char,
    preferred_symbols: &HashMap<char, u16>,
) -> Option<u16> {
    let candidates = switch_ui_char_candidates(ch);
    for candidate in candidates.iter().copied() {
        if let Some(symbol_id) = preferred_symbols.get(&candidate).copied() {
            return Some(symbol_id);
        }
    }
    for candidate in candidates.iter().copied() {
        if let Some(symbol_id) = mcd_symbol_for_char_readonly(symbols, font_id, candidate) {
            return Some(symbol_id);
        }
    }
    for candidate in candidates {
        if let Some(symbol_id) = mcd_symbol_for_char_any_font(symbols, candidate) {
            return Some(symbol_id);
        }
    }
    None
}

fn switch_ui_char_candidates(ch: char) -> Vec<char> {
    let mut candidates = vec![ch];
    if ch.is_ascii_lowercase() {
        candidates.push(ch.to_ascii_uppercase());
    } else if ch.is_ascii_uppercase() {
        candidates.push(ch.to_ascii_lowercase());
    }
    match ch {
        '|' | '‐' | '‑' | '‒' | '–' | '—' | '―' | '−' | '﹣' | '－' => candidates.push('-'),
        '…' | '・' | '•' | '·' => candidates.push('.'),
        _ => {}
    }
    candidates
}

fn mcd_symbol_for_char_readonly(symbols: &[McdSymbol], font_id: u32, ch: char) -> Option<u16> {
    symbols
        .iter()
        .enumerate()
        .rfind(|(_, symbol)| symbol.font_id as u32 == font_id && symbol.ch == ch)
        .map(|(idx, _)| idx as u16)
}

fn mcd_symbol_for_char_any_font(symbols: &[McdSymbol], ch: char) -> Option<u16> {
    symbols
        .iter()
        .enumerate()
        .rfind(|(_, symbol)| symbol.ch == ch)
        .map(|(idx, _)| idx as u16)
}

fn find_switch_ui_data_dir(resources_dir: &Path) -> Option<PathBuf> {
    [
        "switch/data",
        "switch/data009",
        "switch_ui/data",
        "switch_ui/data009",
        "switch_resources/data",
        "switch_resources/data009",
        "switch_ui_only/data",
        "switch_ui_only/data009",
        "data_switch",
    ]
    .into_iter()
    .map(|relative| resources_dir.join(relative))
    .find(|candidate| candidate.join("ui").is_dir())
}

fn read_dat_member(source: &Path, member_name: &str) -> Result<Vec<u8>, String> {
    let bytes =
        std::fs::read(source).map_err(|e| format!("Failed to read {}: {e}", source.display()))?;
    let dat = parse_dat(&bytes)?;
    dat.files
        .into_iter()
        .find(|file| file.name == member_name)
        .map(|file| file.bytes)
        .ok_or_else(|| format!("DAT member {member_name} not found in {}", source.display()))
}

fn repack_dat(
    source: &Path,
    target: &Path,
    replacements: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let bytes =
        std::fs::read(source).map_err(|e| format!("Failed to read {}: {e}", source.display()))?;
    let mut dat = parse_dat(&bytes)?;
    for file in &mut dat.files {
        if let Some((_, replacement)) = replacements.iter().find(|(name, _)| name == &file.name) {
            file.bytes = replacement.clone();
        }
    }
    std::fs::write(target, serialize_dat(&mut dat))
        .map_err(|e| format!("Failed to write {}: {e}", target.display()))
}

fn repack_wtp(
    wta: &mut WtaFile,
    source_wtp: &[u8],
    replacements: &[(usize, Vec<u8>)],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (idx, texture) in wta.textures.iter_mut().enumerate() {
        let bytes = replacements
            .iter()
            .find(|(replacement_idx, _)| *replacement_idx == idx)
            .map(|(_, bytes)| bytes.as_slice())
            .unwrap_or_else(|| {
                let start = texture.offset as usize;
                let end = start + texture.size as usize;
                &source_wtp[start..end]
            });
        texture.offset = out.len() as u32;
        texture.size = bytes.len() as u32;
        out.extend(bytes);
        pad(&mut out, 0x1000);
    }
    Ok(out)
}

fn last_font_texture_page(font_dtt_dir: &Path) -> Result<u16, String> {
    let mut pages = Vec::new();
    for entry in std::fs::read_dir(font_dtt_dir)
        .map_err(|e| format!("Failed to read {}: {e}", font_dtt_dir.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read font texture entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(number) = name
            .strip_suffix(".dds")
            .and_then(|name| name.rsplit_once('_').map(|(_, n)| n))
            .and_then(|n| n.parse::<u16>().ok())
        {
            pages.push(number);
        }
    }
    pages
        .into_iter()
        .max()
        .ok_or_else(|| format!("No DDS font pages found in {}", font_dtt_dir.display()))
}

fn run_magick(command: &str, args: &[&Path], label: &str) -> Result<(), String> {
    let status = Command::new(command)
        .args(args)
        .status()
        .map_err(|e| format!("Failed to run ImageMagick command '{command}' to {label}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "ImageMagick command '{command}' failed while trying to {label} with status {status}"
        ))
    }
}

fn overlay_rgba(target: &mut image::RgbaImage, source: &image::RgbaImage, x: u32, y: u32) {
    for sy in 0..source.height() {
        for sx in 0..source.width() {
            target.put_pixel(x + sx, y + sy, *source.get_pixel(sx, sy));
        }
    }
}

fn parse_dat(bytes: &[u8]) -> Result<DatFile, String> {
    let mut c = Cursor::new(bytes);
    let magic = read_array::<4, _>(&mut c)?;
    if &magic != b"DAT\0" {
        return Err("Invalid DAT magic".into());
    }
    let file_count = read_u32_le(&mut c)? as usize;
    let file_table_offset = read_u32_le(&mut c)? as usize;
    let _ext_table_offset = read_u32_le(&mut c)? as usize;
    let name_table_offset = read_u32_le(&mut c)? as usize;
    let size_table_offset = read_u32_le(&mut c)? as usize;
    let crc_table_offset = read_u32_le(&mut c)? as usize;
    let unknown = read_array::<4, _>(&mut c)?;

    c.set_position(file_table_offset as u64);
    let offsets: Vec<usize> = (0..file_count)
        .map(|_| read_u32_le(&mut c).map(|v| v as usize))
        .collect::<Result<_, _>>()?;

    c.set_position(size_table_offset as u64);
    let sizes: Vec<usize> = (0..file_count)
        .map(|_| read_u32_le(&mut c).map(|v| v as usize))
        .collect::<Result<_, _>>()?;

    c.set_position(name_table_offset as u64);
    let filename_len = read_u32_le(&mut c)? as usize;
    let names: Vec<String> = (0..file_count)
        .map(|_| read_fixed_utf8(&mut c, filename_len))
        .collect::<Result<_, _>>()?;

    let crc_end = offsets.first().copied().unwrap_or(crc_table_offset);
    let crc_table = bytes
        .get(crc_table_offset..crc_end)
        .ok_or_else(|| "Invalid DAT CRC table range".to_string())?
        .to_vec();

    let mut files = Vec::new();
    for ((name, offset), size) in names.into_iter().zip(offsets).zip(sizes) {
        let data = bytes
            .get(offset..offset + size)
            .ok_or_else(|| format!("Invalid DAT file range for {name}"))?
            .to_vec();
        files.push(DatEntry { name, bytes: data });
    }

    Ok(DatFile {
        magic,
        unknown,
        crc_table,
        files,
    })
}

fn serialize_dat(dat: &mut DatFile) -> Vec<u8> {
    let file_count = dat.files.len();
    let mut out = vec![0; 32];
    let file_table_offset = out.len();
    out.resize(out.len() + 4 * file_count, 0);

    let ext_table_offset = out.len();
    for file in &dat.files {
        write_fixed_utf8(&mut out, file.name.rsplit('.').next().unwrap_or(""), 4);
    }

    let name_table_offset = out.len();
    let filename_len = dat.files.iter().map(|f| f.name.len()).max().unwrap_or(0) + 1;
    write_u32_le_vec(&mut out, filename_len as u32);
    for file in &dat.files {
        write_fixed_utf8(&mut out, &file.name, filename_len);
    }
    pad(&mut out, 4);

    let size_table_offset = out.len();
    for file in &dat.files {
        write_u32_le_vec(&mut out, file.bytes.len() as u32);
    }

    let crc_table_offset = out.len();
    out.extend(&dat.crc_table);
    pad(&mut out, 16);

    let mut offset_table = Vec::new();
    for file in &dat.files {
        write_u32_le_vec(&mut offset_table, out.len() as u32);
        out.extend(&file.bytes);
        pad(&mut out, 16);
    }

    out[0..4].copy_from_slice(&dat.magic);
    out[4..8].copy_from_slice(&(file_count as u32).to_le_bytes());
    out[8..12].copy_from_slice(&(file_table_offset as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(ext_table_offset as u32).to_le_bytes());
    out[16..20].copy_from_slice(&(name_table_offset as u32).to_le_bytes());
    out[20..24].copy_from_slice(&(size_table_offset as u32).to_le_bytes());
    out[24..28].copy_from_slice(&(crc_table_offset as u32).to_le_bytes());
    out[28..32].copy_from_slice(&dat.unknown);
    out[file_table_offset..file_table_offset + offset_table.len()].copy_from_slice(&offset_table);
    out
}

fn put_smd_strings(bytes: &[u8], mapping: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    let mut c = Cursor::new(bytes);
    let count = read_u32_le(&mut c)? as usize;
    let mut out = Vec::new();
    write_u32_le_vec(&mut out, count as u32);
    for _ in 0..count {
        let id = read_utf16_fixed(&mut c, 0x80)?;
        let number = read_bytes(&mut c, 8)?;
        let _text = read_utf16_fixed(&mut c, 0x800)?;
        let text = mapping
            .get(&id)
            .ok_or_else(|| format!("Missing SMD mapping for {id}"))?;
        write_utf16_fixed(&mut out, &id, 0x80);
        out.extend(number);
        write_utf16_fixed(&mut out, text, 0x800);
    }
    Ok(out)
}

fn put_tmd_strings(bytes: &[u8], mapping: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    let mut c = Cursor::new(bytes);
    let count = read_u32_le(&mut c)? as usize;
    let mut entries = Vec::new();
    for _ in 0..count {
        let id_len = read_u32_le(&mut c)? as usize * 2;
        let id = read_utf16_fixed(&mut c, id_len)?;
        let text_len = read_u32_le(&mut c)? as usize * 2;
        let _text = read_utf16_fixed(&mut c, text_len)?;
        let text = mapping
            .get(&id)
            .ok_or_else(|| format!("Missing TMD mapping for {id}"))?
            .clone();
        entries.push((id, text));
    }
    let mut out = Vec::new();
    write_u32_le_vec(&mut out, count as u32);
    for (id, text) in entries {
        write_u32_le_vec(&mut out, (id.encode_utf16().count() + 1) as u32);
        write_utf16_fixed(&mut out, &id, (id.encode_utf16().count() + 1) * 2);
        write_u32_le_vec(&mut out, (text.encode_utf16().count() + 1) as u32);
        write_utf16_fixed(&mut out, &text, (text.encode_utf16().count() + 1) * 2);
    }
    Ok(out)
}

#[derive(Deserialize)]
struct McdFontJson {
    glyphs: String,
    #[serde(default)]
    kerning: Option<String>,
    below: f32,
}

struct McdRuntimeFont {
    id: u32,
    glyphs_dir: PathBuf,
    kerning_table: Option<KtbFile>,
    below: f32,
    horiz: f32,
}

impl McdRuntimeFont {
    fn glyph_path(&self, ch: char) -> PathBuf {
        self.glyphs_dir.join(format!("{:04x}.png", ch as u32))
    }

    fn kerning(&self, left: char, right: char) -> u16 {
        self.kerning_table
            .as_ref()
            .map(|table| table.get_kerning(left, right))
            .unwrap_or(0)
    }
}

fn load_mcd_fonts(
    fonts_json_path: &Path,
    resources_dir: &Path,
) -> Result<HashMap<u32, McdRuntimeFont>, String> {
    let content = std::fs::read_to_string(fonts_json_path)
        .map_err(|e| format!("Failed to read {}: {e}", fonts_json_path.display()))?;
    let raw: BTreeMap<String, McdFontJson> = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {e}", fonts_json_path.display()))?;
    let fonts_base = fonts_json_path
        .parent()
        .ok_or_else(|| format!("Invalid fonts.json path {}", fonts_json_path.display()))?;
    let mut fonts = HashMap::new();
    for (id_raw, font) in raw {
        let id = id_raw
            .parse::<u32>()
            .map_err(|e| format!("Invalid MCD font id '{id_raw}' in fonts.json: {e}"))?;
        let glyphs_dir = resolve_nier_resource_path(resources_dir, fonts_base, &font.glyphs);
        if !glyphs_dir.is_dir() {
            return Err(format!(
                "MCD font {} glyph directory does not exist: {}",
                id,
                glyphs_dir.display()
            ));
        }
        let kerning_table = font
            .kerning
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                let path = resolve_nier_resource_path(resources_dir, fonts_base, value);
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
                KtbFile::parse(&bytes)
            })
            .transpose()?;
        fonts.insert(
            id,
            McdRuntimeFont {
                id,
                glyphs_dir,
                kerning_table,
                below: font.below,
                horiz: 0.0,
            },
        );
    }
    Ok(fonts)
}

fn resolve_nier_resource_path(resources_dir: &Path, fonts_base: &Path, value: &str) -> PathBuf {
    let raw = Path::new(value);
    if raw.is_absolute() {
        return raw.to_path_buf();
    }
    let from_resources = resources_dir.join(raw);
    if from_resources.exists() {
        return from_resources;
    }
    let from_fonts_base = fonts_base.join(raw);
    if from_fonts_base.exists() {
        return from_fonts_base;
    }
    if let Some(stripped) = value.strip_prefix("fonts/") {
        return resources_dir.join("fonts").join(stripped);
    }
    if let Some(stripped) = value.strip_prefix("assembly/") {
        let from_unpacked = resources_dir.join("unpacked").join(stripped);
        if from_unpacked.exists() {
            return from_unpacked;
        }
    }
    from_resources
}

fn put_mcd_strings_with_texture(
    options: &NierPackOptions,
    job: &BuildJob,
    bytes: &[u8],
    mapping: &BTreeMap<String, String>,
    fonts: &HashMap<u32, McdRuntimeFont>,
    work_dir: &Path,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    let dtt_file = mcd_dtt_file(&job.dat_file)?;
    let dtt_unpacked_dir = options.resources_dir.join("unpacked").join(&dtt_file);
    if has_astc_textures(&dtt_unpacked_dir)? {
        return Err(format!(
            "UI MCD texture packing for ASTC archives is not implemented yet: {}",
            dtt_unpacked_dir.display()
        ));
    }

    let mut mcd = McdFile::parse(bytes)?;
    let texture = generate_mcd_texture(&mut mcd, mapping, fonts)?;
    mcd.put_strings_with_fonts(mapping, fonts)?;
    let patched_mcd = mcd.serialize()?;

    let _ = std::fs::remove_dir_all(work_dir);
    std::fs::create_dir_all(work_dir)
        .map_err(|e| format!("Failed to create {}: {e}", work_dir.display()))?;
    let texture_png = work_dir.join(format!("{}.wtp_000.png", job.name));
    texture
        .save(&texture_png)
        .map_err(|e| format!("Failed to write {}: {e}", texture_png.display()))?;
    let texture_dds = work_dir.join(format!("{}.wtp_000.dds", job.name));
    run_magick(
        "magick",
        &[
            Path::new("-define"),
            Path::new("dds:mipmaps=0"),
            texture_png.as_path(),
            texture_dds.as_path(),
        ],
        "convert patched Nier UI MCD texture PNG to DDS",
    )?;

    let wta_path = options
        .resources_dir
        .join("unpacked")
        .join(&job.dat_file)
        .join(format!("{}.wta", job.name));
    let wtp_path = dtt_unpacked_dir.join(format!("{}.wtp", job.name));
    let mut wta = WtaFile::parse(
        &std::fs::read(&wta_path)
            .map_err(|e| format!("Failed to read {}: {e}", wta_path.display()))?,
    )?;
    let patched_wtp = repack_wtp(
        &mut wta,
        &std::fs::read(&wtp_path)
            .map_err(|e| format!("Failed to read {}: {e}", wtp_path.display()))?,
        &[(
            0,
            std::fs::read(&texture_dds)
                .map_err(|e| format!("Failed to read {}: {e}", texture_dds.display()))?,
        )],
    )?;

    Ok((patched_mcd, wta.serialize(), patched_wtp))
}

fn mcd_dtt_file(dat_file: &str) -> Result<String, String> {
    dat_file
        .strip_suffix(".dat")
        .map(|base| format!("{base}.dtt"))
        .ok_or_else(|| format!("MCD archive name is not a .dat file: {dat_file}"))
}

fn has_astc_textures(dir: &Path) -> Result<bool, String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("Failed to read {}: {e}", dir.display()))?
    {
        let path = entry
            .map_err(|e| format!("Failed to read directory entry in {}: {e}", dir.display()))?
            .path();
        if path
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("astc"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn safe_work_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

struct McdGlyphRequest {
    ch: char,
    font_id: u32,
    glyph_path: PathBuf,
}

struct McdGlyphImage {
    path_key: String,
    image: image::RgbaImage,
}

fn generate_mcd_texture(
    mcd: &mut McdFile,
    mapping: &BTreeMap<String, String>,
    fonts: &HashMap<u32, McdRuntimeFont>,
) -> Result<image::RgbaImage, String> {
    let requests = mcd_glyph_requests(mcd, mapping, fonts)?;
    let mut unique_paths = HashSet::new();
    let mut images = Vec::new();
    for request in &requests {
        let path_key = request.glyph_path.to_string_lossy().to_string();
        if unique_paths.insert(path_key.clone()) {
            let image = image::open(&request.glyph_path)
                .map_err(|e| format!("Failed to open {}: {e}", request.glyph_path.display()))?
                .to_rgba8();
            images.push(McdGlyphImage { path_key, image });
        }
    }
    if images.is_empty() {
        return Err("No glyph PNGs were selected for generated Nier UI MCD texture.".into());
    }

    images.sort_by(|a, b| {
        b.image
            .height()
            .cmp(&a.image.height())
            .then_with(|| b.image.width().cmp(&a.image.width()))
            .then_with(|| a.path_key.cmp(&b.path_key))
    });

    let mut page = McdPageGenerator::new();
    let mut x_zero = 0u32;
    let mut y_zero = 0u32;
    let mut texture_width = 0u32;
    let mut texture_height = 0u32;
    let mut x = 1u32;
    let mut y = 1u32;
    let mut next_row = 0u32;
    let mut positions: HashMap<String, (u32, u32)> = HashMap::new();

    for glyph in &images {
        if x + glyph.image.width() > texture_width {
            x = x_zero;
            y = next_row;
        }
        if y + glyph.image.height() > texture_height {
            let rect = page.next_rect();
            x_zero = rect.0;
            y_zero = rect.1;
            texture_width = rect.2;
            texture_height = rect.3;
            x = x_zero;
            y = y_zero;
            next_row = y + 1;
        }
        positions.insert(glyph.path_key.clone(), (x, y));
        x += glyph.image.width();
        next_row = next_row.max(y + glyph.image.height());
    }

    let mut texture = image::RgbaImage::from_pixel(
        texture_width,
        texture_height,
        image::Rgba([0, 0, 0, 0]),
    );
    for glyph in &images {
        let (x, y) = positions
            .get(&glyph.path_key)
            .copied()
            .ok_or_else(|| format!("Missing generated MCD glyph position for {}", glyph.path_key))?;
        overlay_rgba(&mut texture, &glyph.image, x, y);
    }

    mcd.remove_all_glyphs();
    let image_by_path = images
        .iter()
        .map(|glyph| (glyph.path_key.as_str(), &glyph.image))
        .collect::<HashMap<_, _>>();
    for request in &requests {
        let path_key = request.glyph_path.to_string_lossy().to_string();
        let image = image_by_path
            .get(path_key.as_str())
            .ok_or_else(|| format!("Missing generated MCD glyph image for {path_key}"))?;
        let (x, y) = positions
            .get(&path_key)
            .copied()
            .ok_or_else(|| format!("Missing generated MCD glyph position for {path_key}"))?;
        mcd.put_glyph(
            request.ch,
            request.font_id,
            x,
            y,
            image.width(),
            image.height(),
            texture_width,
            texture_height,
        )?;
    }

    Ok(texture)
}

fn mcd_glyph_requests(
    mcd: &McdFile,
    mapping: &BTreeMap<String, String>,
    fonts: &HashMap<u32, McdRuntimeFont>,
) -> Result<Vec<McdGlyphRequest>, String> {
    let mut chars_per_font: HashMap<u32, HashSet<char>> =
        fonts.keys().map(|id| (*id, HashSet::new())).collect();
    let fonts_by_event = mcd.fonts_for_messages()?;
    for (event, font_ids) in fonts_by_event {
        let value = mapping
            .get(&event)
            .ok_or_else(|| format!("Missing MCD mapping for {event}"))?;
        let chars = mcd_visible_chars(value);
        for font_id in font_ids {
            let target = chars_per_font
                .get_mut(&font_id)
                .ok_or_else(|| format!("MCD font {font_id} is not present in fonts.json"))?;
            target.extend(chars.iter().copied());
        }
    }

    let mut requests = Vec::new();
    for (font_id, chars) in chars_per_font {
        let font = fonts
            .get(&font_id)
            .ok_or_else(|| format!("MCD font {font_id} is not present in fonts.json"))?;
        for ch in chars {
            let glyph_path = font.glyph_path(ch);
            if !glyph_path.is_file() {
                return Err(format!(
                    "Missing MCD glyph PNG for '{}' in font {}: {}",
                    ch,
                    font_id,
                    glyph_path.display()
                ));
            }
            requests.push(McdGlyphRequest {
                ch,
                font_id,
                glyph_path,
            });
        }
    }
    requests.sort_by(|a, b| {
        a.font_id
            .cmp(&b.font_id)
            .then_with(|| (a.ch as u32).cmp(&(b.ch as u32)))
    });
    Ok(requests)
}

fn mcd_visible_chars(value: &str) -> Vec<char> {
    let mut result = Vec::new();
    let mut iter = value.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        if value[idx..].starts_with("<special:") {
            if let Some(end) = value[idx..].find('>').map(|offset| idx + offset) {
                while iter.peek().is_some_and(|(next_idx, _)| *next_idx <= end) {
                    iter.next();
                }
            }
            continue;
        }
        if ch != '\n' && ch != '\r' && ch != ' ' {
            result.push(ch);
        }
    }
    result
}

struct McdPageGenerator {
    size: u32,
    phase: u8,
}

impl McdPageGenerator {
    fn new() -> Self {
        Self { size: 256, phase: 0 }
    }

    fn next_rect(&mut self) -> (u32, u32, u32, u32) {
        match self.phase {
            0 => {
                self.phase = 1;
                (0, 0, self.size, self.size)
            }
            1 => {
                self.phase = 2;
                (0, self.size, self.size, self.size * 2)
            }
            _ => {
                let rect = (self.size, 0, self.size * 2, self.size * 2);
                self.size *= 2;
                self.phase = 1;
                rect
            }
        }
    }
}

fn put_bin_strings(bytes: &[u8], mapping: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    let mut file = RiteFile::parse(bytes)?;
    file.put_strings(mapping)?;
    file.serialize()
}

fn put_pak_strings(bytes: &[u8], mapping: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    let mut c = Cursor::new(bytes);
    let mut entries = Vec::new();
    loop {
        let typ = read_i32_le(&mut c)?;
        if typ == 0 {
            break;
        }
        let compressed_size = read_i32_le(&mut c)?;
        let offset = read_i32_le(&mut c)? as usize;
        entries.push((typ, compressed_size, offset));
    }
    let offsets: Vec<usize> = entries.iter().map(|e| e.2).collect();
    let mut segments = Vec::new();
    for (idx, (typ, compressed_size, offset)) in entries.iter().enumerate() {
        let real_size = offsets
            .get(idx + 1)
            .copied()
            .unwrap_or(bytes.len())
            .saturating_sub(*offset);
        segments.push(PakSegment {
            typ: *typ,
            compressed_size: *compressed_size,
            content: bytes
                .get(*offset..offset + real_size)
                .ok_or_else(|| "Invalid PAK segment range".to_string())?
                .to_vec(),
        });
    }

    let strings_id = segments
        .iter()
        .position(|s| s.compressed_size == 115220)
        .unwrap_or(25);
    let segment = segments
        .get_mut(strings_id)
        .ok_or_else(|| format!("PAK strings segment {strings_id} not found"))?;
    let mut decoder = ZlibDecoder::new(
        segment
            .content
            .get(4..)
            .ok_or_else(|| "Invalid PAK zlib segment".to_string())?,
    );
    let mut unzipped = Vec::new();
    decoder
        .read_to_end(&mut unzipped)
        .map_err(|e| format!("Failed to inflate PAK strings segment: {e}"))?;
    let mut charnames = CharNames::parse(&unzipped)?;
    charnames.put_strings(mapping)?;
    let rebuilt = charnames.serialize();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(&rebuilt)
        .map_err(|e| format!("Failed to deflate PAK strings segment: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finish PAK compression: {e}"))?;
    segment.compressed_size = rebuilt.len() as i32;
    segment.content.clear();
    write_u32_le_vec(&mut segment.content, compressed.len() as u32);
    segment.content.extend(compressed);
    pad(&mut segment.content, 4);

    let mut out = Vec::new();
    let mut offset = 12 * segments.len() + 4;
    for segment in &segments {
        write_i32_le_vec(&mut out, segment.typ);
        write_i32_le_vec(&mut out, segment.compressed_size);
        write_i32_le_vec(&mut out, offset as i32);
        offset += segment.content.len();
    }
    write_i32_le_vec(&mut out, 0);
    for segment in &segments {
        out.extend(&segment.content);
    }
    Ok(out)
}

struct PakSegment {
    typ: i32,
    compressed_size: i32,
    content: Vec<u8>,
}

struct CharNames {
    entries: Vec<(u64, String)>,
}

impl CharNames {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor::new(bytes);
        let count = read_u32_le(&mut c)? as usize;
        let mut offsets = Vec::new();
        for _ in 0..count {
            let id = read_u40_le(&mut c)?;
            let offset = read_u32_le(&mut c)? as usize;
            offsets.push((id, offset));
        }
        let mut entries = Vec::<(u64, String)>::new();
        for (id, offset) in offsets {
            if offset == 0 {
                if !entries.iter().any(|(entry_id, _)| *entry_id == id) {
                    entries.push((id, String::new()));
                }
                continue;
            }
            c.set_position(offset as u64);
            let s = read_zero_utf8(&mut c)?;
            if let Some((_, value)) = entries.iter_mut().find(|(entry_id, _)| *entry_id == id) {
                value.push_str(&s);
            } else {
                entries.push((id, s));
            }
        }
        Ok(Self { entries })
    }

    fn put_strings(&mut self, mapping: &BTreeMap<String, String>) -> Result<(), String> {
        let key = 0x1D77583401u64;
        let current = self
            .entries
            .iter()
            .find(|(id, _)| *id == key)
            .map(|(_, value)| value.as_str())
            .ok_or_else(|| "Character names table missing in PAK".to_string())?;
        if current.len() < 40 {
            return Err("Character names table is too short".into());
        }
        let bytes = hex_to_bytes(&current[40..])?;
        let table =
            String::from_utf8(bytes).map_err(|e| format!("Invalid charnames UTF-8: {e}"))?;
        let mut id = String::new();
        let mut lines = Vec::new();
        for mut line in table.split('\n').map(ToString::to_string) {
            if line.len() > 2 {
                let chars: Vec<char> = line.chars().collect();
                if chars.get(1) != Some(&' ') {
                    id = line.trim_matches(' ').to_string();
                } else if line.get(2..5) == Some("ENG") {
                    if let Some(value) = mapping.get(&id) {
                        let prefix: String = line.chars().take(6).collect();
                        line = format!("{prefix}{value}");
                    }
                }
            }
            lines.push(line);
        }
        let new_hex = bytes_to_hex(lines.join("\n").as_bytes());
        let prefix = current[..40].to_string();
        let updated = format!("{prefix}{new_hex}");
        if let Some((_, value)) = self.entries.iter_mut().find(|(id, _)| *id == key) {
            *value = updated;
        }
        let strlen = self
            .entries
            .iter()
            .find(|(id, _)| *id == key)
            .map(|(_, value)| value.len() / 2)
            .unwrap_or(0);
        if let Some((_, value)) = self
            .entries
            .iter_mut()
            .find(|(id, _)| *id == 0xF7C0246A01)
        {
            *value = format!("{strlen:#x}");
        } else {
            self.entries.push((0xF7C0246A01, format!("{strlen:#x}")));
        }
        Ok(())
    }

    fn serialize(&self) -> Vec<u8> {
        let mut strings = Vec::<(u64, Vec<u8>)>::new();
        for (id, value) in &self.entries {
            if value.is_empty() {
                strings.push((*id, Vec::new()));
            } else {
                let bytes = value.as_bytes();
                for chunk in bytes.chunks(64) {
                    let mut c = chunk.to_vec();
                    c.push(0);
                    strings.push((*id, c));
                }
            }
        }
        let mut out = Vec::new();
        write_u32_le_vec(&mut out, strings.len() as u32);
        let mut offset = 4 + 9 * strings.len();
        for (id, value) in &strings {
            write_u40_le_vec(&mut out, *id);
            if value.is_empty() {
                write_u32_le_vec(&mut out, 0);
            } else {
                write_u32_le_vec(&mut out, offset as u32);
            }
            offset += value.len();
        }
        for (_, value) in strings {
            out.extend(value);
        }
        out
    }
}

struct RiteFile {
    version: String,
    compiler: String,
    sections: Vec<RiteSection>,
}

enum RiteSection {
    Irep {
        version: Vec<u8>,
        segments: Vec<IrepSegment>,
    },
    Unknown {
        typ: [u8; 4],
        content: Vec<u8>,
    },
}

struct IrepSegment {
    nlocals: u16,
    nregs: u16,
    rlen: u16,
    instructions: Vec<u32>,
    pool: Vec<PoolRecord>,
    symbols: Vec<String>,
}

struct PoolRecord {
    typ: u8,
    value: String,
}

impl RiteFile {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut c = Cursor::new(bytes);
        if read_bytes(&mut c, 4)? != b"RITE" {
            return Err("Invalid RITE magic".into());
        }
        let version = read_fixed_utf8(&mut c, 4)?;
        let _crc = read_bytes(&mut c, 2)?;
        let size = read_u32_be(&mut c)? as u64;
        let compiler = read_fixed_utf8(&mut c, 8)?;
        let mut sections = Vec::new();
        while c.position() < size {
            let typ = read_array::<4, _>(&mut c)?;
            let section_size = read_u32_be(&mut c)? as u64;
            if &typ == b"IREP" {
                let version = read_bytes(&mut c, 4)?;
                let end = c.position() + section_size - 12;
                let mut segments = Vec::new();
                while c.position() < end {
                    segments.push(IrepSegment::parse(&mut c)?);
                }
                sections.push(RiteSection::Irep { version, segments });
            } else {
                let content = read_bytes(&mut c, section_size as usize - 8)?;
                sections.push(RiteSection::Unknown { typ, content });
            }
        }
        Ok(Self {
            version,
            compiler,
            sections,
        })
    }

    fn put_strings(&mut self, mapping: &BTreeMap<String, String>) -> Result<(), String> {
        for section in &mut self.sections {
            let RiteSection::Irep { segments, .. } = section else {
                continue;
            };
            for segment in segments {
                let len = segment.instructions.len();
                for instr_idx in 0..len {
                    let instr = segment.instructions[instr_idx];
                    if mruby_opcode(instr) != 0x37 || get_a(instr) != 1 || get_b(instr) != 1 {
                        continue;
                    }
                    let array_size = get_c(instr) as usize;
                    if array_size <= 1 || instr_idx < array_size || instr_idx + 1 >= len {
                        continue;
                    }
                    let symbol_instr = segment.instructions[instr_idx + 1];
                    if mruby_opcode(symbol_instr) != 0x12 {
                        continue;
                    }
                    let symbol_idx = get_bx(symbol_instr) as usize;
                    let Some(symbol) = segment.symbols.get(symbol_idx).cloned() else {
                        continue;
                    };
                    let string_instr_idx = instr_idx - array_size + 1;
                    let prev_pool_idx = get_bx(segment.instructions[string_instr_idx]) as usize;
                    let prev = segment
                        .pool
                        .get(prev_pool_idx)
                        .map(|p| p.value.as_str())
                        .unwrap_or("");
                    let Some(value) = mapping.get(&symbol) else {
                        return Err(format!("Missing BIN mapping for {symbol}"));
                    };
                    if value != prev {
                        segment.pool.push(PoolRecord {
                            typ: 0,
                            value: value.clone(),
                        });
                        let pool_id = segment.pool.len() as u32 - 1;
                        segment.instructions[string_instr_idx] = (2 << 23) | (pool_id << 7) | 0x3D;
                    }
                }
            }
        }
        Ok(())
    }

    fn serialize(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend(b"RITE");
        write_fixed_utf8(&mut out, &self.version, 4);
        out.extend([0, 0, 0, 0, 0, 0]);
        write_fixed_utf8(&mut out, &self.compiler, 8);
        for section in &self.sections {
            match section {
                RiteSection::Irep { version, segments } => {
                    let start = out.len();
                    out.extend(b"IREP");
                    out.extend([0; 4]);
                    out.extend(version);
                    for segment in segments {
                        out.extend(segment.serialize(out.len())?);
                    }
                    let size = out.len() - start;
                    out[start + 4..start + 8].copy_from_slice(&(size as u32).to_be_bytes());
                }
                RiteSection::Unknown { typ, content } => {
                    out.extend(typ);
                    write_u32_be_vec(&mut out, (content.len() + 8) as u32);
                    out.extend(content);
                }
            }
        }
        let out_len = out.len() as u32;
        out[10..14].copy_from_slice(&out_len.to_be_bytes());
        let crc = calc_mruby_crc(&out[10..]);
        out[8..10].copy_from_slice(&(crc as u16).to_be_bytes());
        Ok(out)
    }
}

impl IrepSegment {
    fn parse(c: &mut Cursor<&[u8]>) -> Result<Self, String> {
        let _size = read_u32_be(c)?;
        let nlocals = read_u16_be(c)?;
        let nregs = read_u16_be(c)?;
        let rlen = read_u16_be(c)?;
        let ninstructions = read_u32_be(c)? as usize;
        read_padding_cursor(c, 4)?;
        let mut instructions = Vec::new();
        for _ in 0..ninstructions {
            instructions.push(read_u32_be(c)?);
        }
        let npool = read_u32_be(c)? as usize;
        let mut pool = Vec::new();
        for _ in 0..npool {
            let typ = read_bytes(c, 1)?[0];
            let size = read_u16_be(c)? as usize;
            pool.push(PoolRecord {
                typ,
                value: read_fixed_utf8(c, size)?,
            });
        }
        let nsymbols = read_u32_be(c)? as usize;
        let mut symbols = Vec::new();
        for _ in 0..nsymbols {
            let size = read_u16_be(c)?;
            if size == 0xFFFF {
                symbols.push(String::new());
            } else {
                symbols.push(read_fixed_utf8(c, size as usize + 1)?);
            }
        }
        Ok(Self {
            nlocals,
            nregs,
            rlen,
            instructions,
            pool,
            symbols,
        })
    }

    fn serialize(&self, offset: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        out.extend([0; 4]);
        write_u16_be_vec(&mut out, self.nlocals);
        write_u16_be_vec(&mut out, self.nregs);
        write_u16_be_vec(&mut out, self.rlen);
        write_u32_be_vec(&mut out, self.instructions.len() as u32);
        let padding_size = padding_len(offset + out.len(), 4);
        out.extend(vec![0; padding_size]);
        for instr in &self.instructions {
            write_u32_be_vec(&mut out, *instr);
        }
        write_u32_be_vec(&mut out, self.pool.len() as u32);
        for p in &self.pool {
            out.push(p.typ);
            let bytes = p.value.as_bytes();
            write_u16_be_vec(&mut out, bytes.len() as u16);
            out.extend(bytes);
        }
        write_u32_be_vec(&mut out, self.symbols.len() as u32);
        for s in &self.symbols {
            if s.is_empty() {
                write_u16_be_vec(&mut out, 0xFFFF);
            } else {
                write_u16_be_vec(&mut out, s.len() as u16);
                write_fixed_utf8(&mut out, s, s.len() + 1);
            }
        }
        let reported = out.len() + 4 - padding_size;
        out[0..4].copy_from_slice(&(reported as u32).to_be_bytes());
        Ok(out)
    }
}

fn mruby_opcode(value: u32) -> u32 {
    value & 0x7F
}
fn get_bx(value: u32) -> u32 {
    (value >> 7) & 0xFFFF
}
fn get_a(value: u32) -> u32 {
    (value >> 23) & 0x1FF
}
fn get_b(value: u32) -> u32 {
    (value >> 14) & 0x1FF
}
fn get_c(value: u32) -> u32 {
    (value >> 7) & 0x7F
}

fn calc_mruby_crc(bytes: &[u8]) -> u32 {
    const CRC_PATTERN: u32 = 0x11021 << 8;
    const CARRY_BIT: u32 = 0x01000000;
    let mut work = 0u32;
    for byte in bytes {
        work |= *byte as u32;
        for _ in 0..8 {
            work <<= 1;
            if work & CARRY_BIT != 0 {
                work ^= CRC_PATTERN;
            }
        }
    }
    work >> 8
}

fn read_bytes<R: Read>(r: &mut R, len: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0; len];
    r.read_exact(&mut out)
        .map_err(|e| format!("Read failed: {e}"))?;
    Ok(out)
}

fn read_vec<R: Read>(r: &mut R, len: usize) -> Result<Vec<u8>, String> {
    read_bytes(r, len)
}

fn read_array<const N: usize, R: Read>(r: &mut R) -> Result<[u8; N], String> {
    let mut out = [0; N];
    r.read_exact(&mut out)
        .map_err(|e| format!("Read failed: {e}"))?;
    Ok(out)
}

fn read_u16_be<R: Read>(r: &mut R) -> Result<u16, String> {
    Ok(u16::from_be_bytes(read_array::<2, _>(r)?))
}
fn read_u16_le<R: Read>(r: &mut R) -> Result<u16, String> {
    Ok(u16::from_le_bytes(read_array::<2, _>(r)?))
}
fn read_u32_le<R: Read>(r: &mut R) -> Result<u32, String> {
    Ok(u32::from_le_bytes(read_array::<4, _>(r)?))
}
fn read_u32_be<R: Read>(r: &mut R) -> Result<u32, String> {
    Ok(u32::from_be_bytes(read_array::<4, _>(r)?))
}
fn read_i32_le<R: Read>(r: &mut R) -> Result<i32, String> {
    Ok(i32::from_le_bytes(read_array::<4, _>(r)?))
}
fn read_i16_le<R: Read>(r: &mut R) -> Result<i16, String> {
    Ok(i16::from_le_bytes(read_array::<2, _>(r)?))
}
fn read_f32_le<R: Read>(r: &mut R) -> Result<f32, String> {
    Ok(f32::from_le_bytes(read_array::<4, _>(r)?))
}
fn read_u40_le<R: Read>(r: &mut R) -> Result<u64, String> {
    let bytes = read_array::<5, _>(r)?;
    Ok(bytes
        .iter()
        .enumerate()
        .map(|(i, b)| (*b as u64) << (8 * i))
        .sum())
}

fn read_char_u16<R: Read>(r: &mut R) -> Result<char, String> {
    let value = read_u16_le(r)?;
    char::from_u32(value as u32).ok_or_else(|| format!("Invalid UTF-16 character value: {value}"))
}

fn read_mcd_content_u16<R: Read>(r: &mut R) -> Result<u16, String> {
    Ok(read_i16_le(r)? as u16)
}

fn read_fixed_utf8<R: Read>(r: &mut R, len: usize) -> Result<String, String> {
    let bytes = read_bytes(r, len)?;
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec()).map_err(|e| format!("Invalid UTF-8: {e}"))
}

fn read_zero_utf8<R: Read>(r: &mut R) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut buf = [0; 1];
    loop {
        r.read_exact(&mut buf)
            .map_err(|e| format!("Read failed: {e}"))?;
        if buf[0] == 0 {
            break;
        }
        bytes.push(buf[0]);
    }
    String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8: {e}"))
}

fn read_utf16_fixed<R: Read>(r: &mut R, byte_len: usize) -> Result<String, String> {
    let bytes = read_bytes(r, byte_len)?;
    let mut units = Vec::new();
    for chunk in bytes.chunks_exact(2) {
        let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16(&units).map_err(|e| format!("Invalid UTF-16: {e}"))
}

fn read_padding_cursor(c: &mut Cursor<&[u8]>, modulo: u64) -> Result<(), String> {
    let pos = c.position();
    let padding = modulo - (pos % modulo);
    if padding != modulo {
        c.seek(SeekFrom::Current(padding as i64))
            .map_err(|e| format!("Seek failed: {e}"))?;
    }
    Ok(())
}

fn write_u32_le_vec(out: &mut Vec<u8>, value: u32) {
    out.extend(value.to_le_bytes());
}
fn write_u32_le(out: &mut Vec<u8>, value: u32) {
    write_u32_le_vec(out, value);
}
fn write_u16_le(out: &mut Vec<u8>, value: u16) {
    out.extend(value.to_le_bytes());
}
fn write_i32_le_vec(out: &mut Vec<u8>, value: i32) {
    out.extend(value.to_le_bytes());
}
fn write_f32_le(out: &mut Vec<u8>, value: f32) {
    out.extend(value.to_le_bytes());
}
fn write_u16_be_vec(out: &mut Vec<u8>, value: u16) {
    out.extend(value.to_be_bytes());
}
fn write_u32_be_vec(out: &mut Vec<u8>, value: u32) {
    out.extend(value.to_be_bytes());
}
fn write_u40_le_vec(out: &mut Vec<u8>, value: u64) {
    for i in 0..5 {
        out.push(((value >> (8 * i)) & 0xFF) as u8);
    }
}

fn write_char_u16(out: &mut Vec<u8>, value: char) -> Result<(), String> {
    let mut units = [0; 2];
    let encoded = value.encode_utf16(&mut units);
    if encoded.len() != 1 {
        return Err(format!(
            "Cannot write non-BMP character '{value}' into u16 font table"
        ));
    }
    write_u16_le(out, encoded[0]);
    Ok(())
}

fn write_fixed_utf8(out: &mut Vec<u8>, value: &str, len: usize) {
    let bytes = value.as_bytes();
    out.extend(bytes);
    if bytes.len() < len {
        out.extend(vec![0; len - bytes.len()]);
    }
}

fn write_utf16_fixed(out: &mut Vec<u8>, value: &str, byte_len: usize) {
    let mut bytes = Vec::new();
    for unit in value.encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    if bytes.len() < byte_len {
        let padding = byte_len - bytes.len();
        out.extend(bytes);
        out.extend(vec![0; padding]);
    } else {
        out.extend(bytes);
    }
}

fn pad(out: &mut Vec<u8>, modulo: usize) {
    let len = padding_len(out.len(), modulo);
    out.extend(vec![0; len]);
}

fn eager_pad(out: &mut Vec<u8>, modulo: usize) {
    let len = eager_padding_len(out.len(), modulo);
    out.extend(vec![0; len]);
}

fn padding_len(offset: usize, modulo: usize) -> usize {
    let rem = offset % modulo;
    if rem == 0 {
        0
    } else {
        modulo - rem
    }
}

fn eager_padding_len(offset: usize, modulo: usize) -> usize {
    modulo - (offset % modulo)
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("Odd-length hex string".into());
    }
    let mut out = Vec::new();
    for i in (0..hex.len()).step_by(2) {
        out.push(u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| format!("Invalid hex: {e}"))?);
    }
    Ok(out)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn required_archive_members() -> BTreeMap<String, Vec<String>> {
    let mut sources: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for job in parse_build_jobs() {
        if matches!(job.ext.as_str(), "bin" | "pak" | "smd" | "tmd" | "mcd") {
            let entry = sources.entry(job.dat_file).or_default();
            let name = format!("{}.{}", job.name, job.ext);
            if !entry.contains(&name) {
                entry.push(name);
            }
        }
    }
    for id in ["00", "01", "04", "05", "11"] {
        sources.entry(format!("font/font_{id}.dat")).or_default();
        sources.entry(format!("font/font_{id}.dtt")).or_default();
    }
    let dtts = sources.keys().filter(|key| key.starts_with("ui/") && key.ends_with(".dat"))
        .map(|key| key.replace(".dat", ".dtt")).collect::<Vec<_>>();
    for name in dtts {
        sources.entry(name).or_default();
    }
    sources
}

pub fn extract_dat_members(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let dat = parse_dat(bytes)?;
    Ok(dat.files.into_iter().map(|file| (file.name, file.bytes)).collect())
}
