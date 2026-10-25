//! Nintendo Switch subtitle archive builder.
//!
//! The Switch engine fails to reliably initialize some cinematics when the
//! subtitle DATs are installed as loose LayeredFS overlays. Instead package
//! all 208 patched subtitle DATs into the additional data300.cpk and ship the
//! tested 1.0.2 priority IPS. Never include either original game CPK in a mod.
//!
//! The table schema/IDs are metadata only; subtitle bytes come exclusively
//! from the normal translation build, not from a bundled game dump.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const UPDATE_102_BUILD_ID: &str = "e43525f22282a47785aaed2dfff6e2e94d82b129";
pub const UPDATE_102_IPS: &[u8] = include_bytes!("../assets/switch_1_0_2_subtitle_priority.ips");
const SUBTITLE_IDS: &str = include_str!("../assets/switch_subtitle_ids.tsv");
const CPK_SCHEMA: &str = include_str!("../assets/switch_subtitle_cpk_schema.json");
const ALIGN: usize = 2048;
const EXPECTED_SUBTITLES: usize = 208;

#[derive(Deserialize)]
struct Schema {
    header: Vec<Column>,
    toc: Vec<Column>,
    etoc: Vec<Column>,
    tvers: String,
    etoc_timestamp: u64,
}
#[derive(Deserialize)]
struct Column {
    name: String,
    #[serde(rename = "type")]
    kind: u8,
    storage: String,
}
#[derive(Clone)]
enum Field { Number(u64), Text(String) }
type Record = BTreeMap<&'static str, Field>;

fn field<'r>(record: &'r Record, name: &str) -> Option<&'r Field> {
    record.get(name)
}
fn string_value(field: Option<&Field>) -> &str {
    match field { Some(Field::Text(s)) => s, _ => "" }
}
fn number_value(field: Option<&Field>) -> u64 {
    match field { Some(Field::Number(n)) => *n, _ => 0 }
}
fn width(typ:u8) -> Result<usize,String> {
    Ok(match typ {0|1=>1,2|3=>2,4|5|8|10=>4,6|7|9|11=>8,_=>return Err(format!("Unsupported CPK UTF type {typ}"))})
}
fn column_storage(col:&Column)->Result<u8,String> {
    match col.storage.as_str() {"0x10"=>Ok(0x10),"0x30"=>Ok(0x30),"0x50"=>Ok(0x50),_=>Err(format!("Unsupported CPK storage {}",col.storage))}
}
fn be16(v: u16, out:&mut Vec<u8>){out.extend_from_slice(&v.to_be_bytes());}
fn be32(v: u32, out:&mut Vec<u8>){out.extend_from_slice(&v.to_be_bytes());}
fn be64(v: u64, out:&mut Vec<u8>){out.extend_from_slice(&v.to_be_bytes());}
fn as_u32(n: usize) -> Result<u32,String> {u32::try_from(n).map_err(|_|"CPK UTF offset exceeds u32".to_string())}

fn intern(s:&str, strings:&mut Vec<u8>, indices:&mut BTreeMap<String,u32>) -> Result<u32,String> {
    if let Some(v)=indices.get(s) {return Ok(*v);}
    let off=as_u32(strings.len())?;
    strings.extend_from_slice(s.as_bytes());
    strings.push(0);
    indices.insert(s.to_owned(),off);
    Ok(off)
}
fn push_value(field:Option<&Field>, typ:u8, indices:&BTreeMap<String,u32>, out:&mut Vec<u8>) -> Result<(),String> {
    let n=number_value(field);
    match typ {
        0|1=>out.push(u8::try_from(n).map_err(|_|"CPK byte overflow")?),
        2|3=>be16(u16::try_from(n).map_err(|_|"CPK u16 overflow")?,out),
        4|5|8=>be32(u32::try_from(n).map_err(|_|"CPK u32 overflow")?,out),
        6|7|9=>be64(n,out),
        10=>be32(*indices.get(string_value(field)).ok_or("Uninterned CPK string")?,out),
        11=>out.extend_from_slice(&[0;8]),
        _=>return Err(format!("Unsupported CPK field type {typ}")),
    }
    Ok(())
}
fn make_utf(table:&str, columns:&[Column], rows:&[Record]) -> Result<Vec<u8>,String> {
    let mut strings=vec![0];
    let mut indices=BTreeMap::new();
    indices.insert(String::new(),0);
    let table_name=intern(table,&mut strings,&mut indices)?;
    for c in columns {intern(&c.name,&mut strings,&mut indices)?;}
    for c in columns {
        if c.kind==10 && column_storage(c)?==0x30 {intern("",&mut strings,&mut indices)?;}
    }
    for row in rows {
        for c in columns {
            if c.kind==10 && column_storage(c)?==0x50 {
                intern(string_value(field(row,&c.name)),&mut strings,&mut indices)?;
            }
        }
    }
    let mut schema=Vec::new();
    let mut rowlen=0usize;
    for c in columns {
        let st=column_storage(c)?;
        schema.push(st|c.kind);
        be32(*indices.get(&c.name).ok_or("Missing CPK column name")?,&mut schema);
        if st==0x30 {push_value(None,c.kind,&indices,&mut schema)?;}
        if st==0x50 {rowlen=rowlen.checked_add(width(c.kind)?).ok_or("CPK row overflow")?;}
    }
    let mut data=Vec::new();
    for row in rows {
        for c in columns {
            if column_storage(c)?==0x50 {push_value(field(row,&c.name),c.kind,&indices,&mut data)?;}
        }
    }
    if data.len()!=rowlen*rows.len(){return Err("CPK UTF row size mismatch".into());}
    let row_start=32+schema.len();
    let str_start=row_start+data.len();
    let data_start=str_start+strings.len();
    let mut out=Vec::with_capacity(data_start);
    out.extend_from_slice(b"@UTF");
    be32(as_u32(data_start-8)?,&mut out);
    be32(as_u32(row_start-8)?,&mut out);
    be32(as_u32(str_start-8)?,&mut out);
    be32(as_u32(data_start-8)?,&mut out);
    be32(table_name,&mut out);
    be16(u16::try_from(columns.len()).map_err(|_|"Too many CPK columns")?,&mut out);
    be16(u16::try_from(rowlen).map_err(|_|"CPK rows too long")?,&mut out);
    be32(as_u32(rows.len())?,&mut out);
    out.extend(schema);
    out.extend(data);
    out.extend(strings);
    Ok(out)
}
fn chunk(magic:&[u8;4],packet:Vec<u8>)->Vec<u8>{
    let mut out=Vec::with_capacity(16+packet.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(&255u32.to_le_bytes());
    out.extend_from_slice(&(packet.len() as u64).to_le_bytes());
    out.extend(packet);
    out
}
fn align(pos:usize)->Result<usize,String>{pos.checked_add(ALIGN-1).map(|v|v/ALIGN*ALIGN).ok_or("CPK alignment overflow".into())}
fn crc32(bytes:&[u8])->u32{
    let mut crc=0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask=(crc&1).wrapping_neg();
            crc=(crc>>1)^(0xedb8_8320&mask);
        }
    }
    !crc
}
fn number_row(pairs:&[(&'static str,u64)])->Record{
    pairs.iter().map(|(k,v)|(*k,Field::Number(*v))).collect()
}
fn checked_dat_name(name:&str)->bool{
    name.len()==19 && name.starts_with("subtitle") && name.ends_with("_us.dat") &&
        name.as_bytes()[8..12].iter().all(|c|c.is_ascii_hexdigit())
}
fn manifest()->Result<BTreeMap<String,u32>,String>{
    let mut ids=BTreeMap::new();
    for line in SUBTITLE_IDS.lines().filter(|s|!s.starts_with('#') && !s.trim().is_empty()) {
        let (name,id)=line.split_once('\t').ok_or("Malformed subtitle ID manifest")?;
        if !checked_dat_name(name) {return Err(format!("Invalid subtitle manifest filename {name}"));}
        let id=id.parse::<u32>().map_err(|_|format!("Invalid subtitle ID for {name}"))?;
        if ids.insert(name.to_owned(),id).is_some() {return Err(format!("Duplicate subtitle manifest entry {name}"));}
    }
    if ids.len()!=EXPECTED_SUBTITLES {return Err(format!("Expected {EXPECTED_SUBTITLES} subtitle IDs; got {}",ids.len()));}
    Ok(ids)
}

/// Requires exactly the expected 208 translated subtitle DATs. This is a
/// hard gate: never publish a partial or silently unpatched Switch archive.
pub fn build(subtitle_dir:&Path,target:&Path)->Result<usize,String>{
    let ids=manifest()?;
    if !subtitle_dir.is_dir(){return Err(format!("Missing Switch subtitle source: {}",subtitle_dir.display()));}
    let mut files=BTreeMap::<String,Vec<u8>>::new();
    for e in fs::read_dir(subtitle_dir).map_err(|e|e.to_string())? {
        let e=e.map_err(|e|e.to_string())?;
        if !e.file_type().map_err(|e|e.to_string())?.is_file(){return Err("Unexpected directory in subtitle/ build".into());}
        let name=e.file_name().into_string().map_err(|_|"Non-UTF8 subtitle name")?;
        if !ids.contains_key(&name){return Err(format!("Unexpected subtitle file: {name}"));}
        let data=fs::read(e.path()).map_err(|e|format!("Cannot read subtitle {name}: {e}"))?;
        if !data.starts_with(b"DAT\0"){return Err(format!("Not a DAT subtitle: {name}"));}
        if files.insert(name.clone(),data).is_some(){return Err(format!("Duplicate subtitle {name}"));}
    }
    if files.len()!=EXPECTED_SUBTITLES {
        let missing=ids.keys().filter(|n|!files.contains_key(*n)).take(12).cloned().collect::<Vec<_>>();
        return Err(format!("Switch packaging requires all {EXPECTED_SUBTITLES} subtitle DATs; got {}. Missing examples: {}",files.len(),missing.join(", ")));
    }
    let schema:Schema=serde_json::from_str(CPK_SCHEMA).map_err(|e|format!("Invalid bundled CPK schema: {e}"))?;
    if schema.header.len()!=44 || schema.toc.len()!=8 || schema.etoc.len()!=2 {
        return Err("Unexpected original Switch CPK schema".into());
    }
    let mut proto=Vec::new();
    for (name,data) in &files {
        let mut row=number_row(&[("FileSize",data.len() as u64),("ExtractSize",data.len() as u64),
            ("FileOffset",0),("ID",ids[name] as u64),("CRC",crc32(data) as u64)]);
        row.insert("DirName",Field::Text("subtitle".to_owned()));
        row.insert("FileName",Field::Text(name.to_owned()));
        proto.push(row);
    }
    let toc_offset=ALIGN;
    let mock_toc=chunk(b"TOC ",make_utf("CpkTocInfoEx",&schema.toc,&proto)?);
    let content_offset=align(toc_offset+mock_toc.len())?;
    let mut positions=Vec::new();
    let mut pos=content_offset;
    for (row,(_,dat)) in proto.iter_mut().zip(&files) {
        row.insert("FileOffset",Field::Number((pos-toc_offset) as u64));
        positions.push(pos);
        pos=align(pos.checked_add(dat.len()).ok_or("CPK payload overflow")?)?;
    }
    let toc=chunk(b"TOC ",make_utf("CpkTocInfoEx",&schema.toc,&proto)?);
    if toc.len()!=mock_toc.len(){return Err("CPK TOC size changed after offsets".into());}
    let etoc_offset=align(pos)?;
    let mut erows=Vec::new();
    for _ in 0..files.len() {
        let mut row=number_row(&[("UpdateDateTime",schema.etoc_timestamp)]);
        row.insert("LocalDir",Field::Text("subtitle".to_owned()));
        erows.push(row);
    }
    let mut final_row=number_row(&[("UpdateDateTime",schema.etoc_timestamp)]);
    final_row.insert("LocalDir",Field::Text(String::new()));
    erows.push(final_row);
    let etoc=chunk(b"ETOC",make_utf("CpkEtocInfo",&schema.etoc,&erows)?);
    let total_size=etoc_offset.checked_add(etoc.len()).ok_or("CPK size overflow")?;
    let byte_sum=files.values().map(|v|v.len() as u64).sum::<u64>();
    let mut header=number_row(&[
        ("UpdateDateTime",1),("ContentOffset",content_offset as u64),
        ("ContentSize",(etoc_offset-content_offset) as u64),
        ("TocOffset",toc_offset as u64),("TocSize",toc.len() as u64),
        ("TocCrc",0),("EtocOffset",etoc_offset as u64),
        ("EtocSize",etoc.len() as u64),("EnabledPackedSize",byte_sum),
        ("EnabledDataSize",byte_sum),("Files",files.len() as u64),
        ("Groups",0),("Attrs",0),("Version",7),("Revision",14),
        ("Align",2048),("Sorted",1),("EnableFileName",1),("CpkMode",1),
        ("Codec",0),("DpkItoc",0),("EnableTocCrc",0),
        ("EnableFileCrc",0),("CrcMode",0),
    ]);
    header.insert("Tvers",Field::Text(schema.tvers.clone()));
    let header=chunk(b"CPK ",make_utf("CpkHeader",&schema.header,&[header])?);
    if header.len()>ALIGN-6 {return Err("CPK header exceeds 2042 bytes".into());}
    let mut bytes=vec![0;total_size];
    bytes[..header.len()].copy_from_slice(&header);
    bytes[ALIGN-6..ALIGN].copy_from_slice(b"(c)CRI");
    bytes[toc_offset..toc_offset+toc.len()].copy_from_slice(&toc);
    for (offset,(_,data)) in positions.iter().zip(&files) {
        bytes[*offset..*offset+data.len()].copy_from_slice(data);
    }
    bytes[etoc_offset..etoc_offset+etoc.len()].copy_from_slice(&etoc);
    if let Some(parent)=target.parent() {fs::create_dir_all(parent).map_err(|e|e.to_string())?;}
    fs::write(target,&bytes).map_err(|e|format!("Cannot write {}: {e}",target.display()))?;
    verify(target,&files,&ids)?;
    Ok(files.len())
}

/// Independent on-disk check with the repository's existing CPK parser.
fn verify(cpk:&Path, files:&BTreeMap<String,Vec<u8>>, ids:&BTreeMap<String,u32>)->Result<(),String>{
    let archive=crate::cpk::index(cpk)?;
    if archive.len()!=EXPECTED_SUBTITLES {return Err(format!("Generated CPK entry count mismatch: {}",archive.len()));}
    let mut seen=BTreeSet::new();
    let mut handle=fs::File::open(cpk).map_err(|e|e.to_string())?;
    for item in archive {
        let name=item.path.strip_prefix("subtitle/").ok_or("Generated CPK path outside subtitle")?;
        let expected=files.get(name).ok_or(format!("Unexpected archive member {name}"))?;
        if !ids.contains_key(name) || !seen.insert(name.to_owned()) || item.unpacked_size!=expected.len() as u64 {
            return Err(format!("Unexpected or duplicated CPK member {name}"));
        }
        if crate::cpk::read_entry(&mut handle,&item)? != *expected {
            return Err(format!("CPK roundtrip mismatch: {name}"));
        }
    }
    if seen.len()!=EXPECTED_SUBTITLES {return Err("Not all subtitles verified in output CPK".into());}
    Ok(())
}

/// Check that required Switch packaging assets are available.
pub fn static_asset_status() -> Result<String, String> {
    let ids = manifest()?;
    if ids.len() != EXPECTED_SUBTITLES {
        return Err(format!("Expected {EXPECTED_SUBTITLES} subtitle IDs"));
    }
    let schema: Schema = serde_json::from_str(CPK_SCHEMA)
        .map_err(|e| format!("Invalid Switch CPK schema: {e}"))?;
    if schema.header.len() != 44 || schema.toc.len() != 8 || schema.etoc.len() != 2 {
        return Err("Invalid Switch CPK schema column counts".into());
    }
    if UPDATE_102_IPS.len() != 65 || !UPDATE_102_IPS.starts_with(b"IPS32")
        || !UPDATE_102_IPS.ends_with(b"EEOF") {
        return Err("Invalid update 1.0.2 IPS32 asset".into());
    }
    Ok(format!(
        "Switch subtitle packaging ready\nbuild_id={}\nips_bytes={}\nsubtitle_entries={}\npackaging=romfs/data300.cpk+exefs/{}.ips",
        UPDATE_102_BUILD_ID, UPDATE_102_IPS.len(), ids.len(), UPDATE_102_BUILD_ID
    ))
}

pub fn write_update_102_ips(exefs:&Path)->Result<PathBuf,String>{
    if UPDATE_102_IPS.len()!=65 || !UPDATE_102_IPS.starts_with(b"IPS32") || !UPDATE_102_IPS.ends_with(b"EEOF") {
        return Err("Invalid bundled update 1.0.2 priority IPS32".into());
    }
    fs::create_dir_all(exefs).map_err(|e|e.to_string())?;
    let path=exefs.join(format!("{}.ips",UPDATE_102_BUILD_ID));
    fs::write(&path,UPDATE_102_IPS).map_err(|e|e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_and_ips_consistent(){
        let m=manifest().unwrap();
        assert_eq!(m.len(),208);
        assert_eq!(m["subtitle1400_us.dat"],3664);
        assert_eq!(m["subtitle1402_us.dat"],3675);
        assert_eq!(UPDATE_102_IPS.len(),65);
    }
    #[test]
    fn cpk_roundtrip_synthetic_208(){
        let p=std::env::temp_dir().join(format!("nier-subtitle-test-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let subtitle=p.join("subtitle");
        fs::create_dir_all(&subtitle).unwrap();
        for name in manifest().unwrap().keys() {
            let mut dat=b"DAT\0".to_vec();
            dat.extend_from_slice(name.as_bytes());
            fs::write(subtitle.join(name),dat).unwrap();
        }
        let cpk=p.join("data300.cpk");
        assert_eq!(build(&subtitle,&cpk).unwrap(),208);
        assert_eq!(crate::cpk::index(&cpk).unwrap().len(),208);
        fs::remove_dir_all(p).unwrap();
    }
}
