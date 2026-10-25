use crate::{cpk, nier_pack};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

#[derive(Default, Debug)]
pub struct PrepareReport {
    pub archives_ready: usize,
    pub members_ready: usize,
    pub cache_hits: usize,
    pub absent: Vec<String>,
    pub notes: Vec<String>,
}

fn sha256(path: &Path) -> Result<Vec<u8>,String> {
    let mut input=File::open(path).map_err(|e|e.to_string())?;
    let mut hash=Sha256::new();
    let mut buffer=[0u8;65536];
    loop {
        let n=input.read(&mut buffer).map_err(|e|e.to_string())?;
        if n==0 {break;}
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().to_vec())
}
fn same_file(input: &Path,output: &Path)->bool {
    let (Ok(left),Ok(right))=(fs::metadata(input),fs::metadata(output)) else {return false;};
    left.len()==right.len() && sha256(input).ok()==sha256(output).ok()
}
fn store_if_changed(input:&Path,output:&Path)->Result<bool,String>{
    if same_file(input,output) {return Ok(false);}
    if let Some(parent)=output.parent(){fs::create_dir_all(parent).map_err(|e|e.to_string())?;}
    let temp=output.with_extension("cache.tmp");
    fs::copy(input,&temp).map_err(|e|format!("{}: {e}",input.display()))?;
    if output.exists(){fs::remove_file(output).map_err(|e|e.to_string())?;}
    fs::rename(temp,output).map_err(|e|e.to_string())?;
    Ok(true)
}
fn store_bytes(bytes:&[u8],output:&Path)->Result<bool,String>{
    if let Ok(existing)=fs::read(output) {if existing==bytes{return Ok(false);}}
    if let Some(parent)=output.parent(){fs::create_dir_all(parent).map_err(|e|e.to_string())?;}
    let temp=output.with_extension("cache.tmp");
    fs::write(&temp,bytes).map_err(|e|e.to_string())?;
    if output.exists(){fs::remove_file(output).map_err(|e|e.to_string())?;}
    fs::rename(temp,output).map_err(|e|e.to_string())?;
    Ok(true)
}

pub fn prepare_from_install(game_dir:&Path,resource_dir:&Path)->Result<PrepareReport,String>{
    let mut report=PrepareReport::default();
    let source=if game_dir.join("data").is_dir() {game_dir.join("data")} else {game_dir.to_path_buf()};
    if !source.is_dir(){return Err(format!("Not a game data directory: {}",source.display()));}
    let required=nier_pack::required_archive_members();
    let mut missing: BTreeSet<String>=required.keys().cloned().collect();
    for rel in required.keys() {
        let source_file=source.join(rel);
        if source_file.is_file(){
            let target=resource_dir.join("data").join(rel);
            if store_if_changed(&source_file,&target)? {report.archives_ready+=1;} else {report.cache_hits+=1;}
            missing.remove(rel);
        }
    }
    // CRI CPK fallback: extract only required archive entries, not complete game files.
    let cpks=fs::read_dir(&source).map_err(|e|e.to_string())?
        .filter_map(|e|e.ok().map(|x|x.path()))
        .filter(|p|p.extension().is_some_and(|e|e.to_string_lossy().eq_ignore_ascii_case("cpk")))
        .collect::<Vec<_>>();
    for archive in cpks {
        if missing.is_empty(){break;}
        let entries=match cpk::index(&archive){
            Ok(entries)=>entries,
            Err(err)=>{report.notes.push(format!("{}: {err}",archive.display()));continue;}
        };
        let mut file=File::open(&archive).map_err(|e|e.to_string())?;
        for entry in entries {
            if !missing.contains(&entry.path) {continue;}
            let raw=cpk::read_entry(&mut file,&entry)?;
            if !raw.starts_with(b"DAT\0") {
                return Err(format!("Invalid DAT in {}: {}",archive.display(),entry.path));
            }
            let dest=resource_dir.join("data").join(&entry.path);
            if store_bytes(&raw,&dest)? {report.archives_ready+=1;} else {report.cache_hits+=1;}
            missing.remove(&entry.path);
        }
    }
    // Reuse previous extracted members when the original archive has not changed.
    for (rel,members) in required {
        let src=resource_dir.join("data").join(&rel);
        if !src.is_file(){continue;}
        let out=resource_dir.join("unpacked").join(&rel);
        let mut should_unpack=false;
        for name in &members {if !out.join(name).is_file(){should_unpack=true;break;}}
        // Font DAT/DTT containers need their internal members for font work.
        // UI DTT and other empty-manifest containers only need to be cached.
        let unpack_all = members.is_empty() && rel.starts_with("font/");
        if members.is_empty() && !unpack_all {continue;}
        if unpack_all && !out.is_dir() {should_unpack=true;}
        // Cache invalidation is based on the original archive digest.
        let digest=sha256(&src)?;
        let marker=out.join(".source_sha256");
        if fs::read(&marker).ok()!=Some(digest.clone()){should_unpack=true;}
        if !should_unpack {report.cache_hits+=1;continue;}
        let input=fs::read(&src).map_err(|e|e.to_string())?;
        let unpacked=nier_pack::extract_dat_members(&input)?;
        fs::create_dir_all(&out).map_err(|e|e.to_string())?;
        let by_name: BTreeMap<_,_>=unpacked.into_iter().collect();
        for name in &members {
            let bytes=by_name.get(name).ok_or_else(||format!("Required {} not found in {}",name,rel))?;
            if store_bytes(bytes,&out.join(name))? {report.members_ready+=1;} else {report.cache_hits+=1;}
        }
        // Font workflows need all members; archive members from .dtt may include WTP.
        if rel.starts_with("font/") {
            for (name,bytes) in &by_name {
                if store_bytes(bytes,&out.join(name))? {report.members_ready+=1;} else {report.cache_hits+=1;}
            }
        }
        fs::write(marker,&digest).map_err(|e|e.to_string())?;
    }
    report.absent=missing.into_iter().collect();
    let font_manifest=resource_dir.join("fonts/fonts.json");
    if !font_manifest.is_file() {
        let bundled=Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts.json");
        if bundled.is_file() { let _=store_if_changed(&bundled,&font_manifest)?; }
    }
    for id in ["00","01","02","03","04","05","08","09","10","11"] {
        if !resource_dir.join("fonts").join(id).is_dir() {
            report.notes.push(format!("Font glyph cache {id} is not prepared. Fresh-install glyph extraction is not available yet."));
        }
    }
    if !resource_dir.join("fonts/fonts.json").is_file() {
        report.notes.push("PC UI font glyph cache is missing (fonts/fonts.json). PC UI MCD rebuild requires prepared font glyphs; Prepared glyphs must be supplied or generated from game resources.".into());
    }
    Ok(report)
}

#[derive(Clone, Debug, Default)]
pub struct CacheAudit {
    pub archives_total: usize,
    pub archives_present: usize,
    pub members_total: usize,
    pub members_present: usize,
    pub font_ready: usize,
    pub font_total: usize,
    pub font_manifest: bool,
    pub switch_ui_dat: usize,
    pub switch_ui_dtt: usize,
    pub switch_ui_required: usize,
    pub switch_ui_missing: Vec<String>,
    pub missing: Vec<String>,
    pub verified: usize,
    pub corrupt: Vec<String>,
}

impl CacheAudit {
    pub fn pc_ready(&self) -> bool {
        self.missing.is_empty() && self.corrupt.is_empty()
    }
    pub fn switch_ui_ready(&self) -> bool {
        self.switch_ui_required > 0 && self.switch_ui_missing.is_empty()
    }
    pub fn status(&self) -> String {
        let pc = if self.pc_ready() { "READY" } else { "INCOMPLETE" };
        let switch = if self.switch_ui_ready() { "READY" } else { "INCOMPLETE" };
        format!(
            "PC source cache: {pc} | {} / {} archives | {} / {} required members\nFonts: {} / {} glyph families | font manifest: {}\nSwitch UI: {switch} | {} / {} DAT and {} / {} DTT\nMissing PC: {} | Missing Switch UI: {} | Verified members: {} | Mismatches: {}",
            self.archives_present,self.archives_total,self.members_present,self.members_total,
            self.font_ready,self.font_total,if self.font_manifest { "yes" } else { "no" },
            self.switch_ui_dat,self.switch_ui_required,self.switch_ui_dtt,self.switch_ui_required,
            self.missing.len(),self.switch_ui_missing.len(),self.verified,self.corrupt.len()
        )
    }
    pub fn issues(&self) -> Vec<String> {
        let mut issues = self.missing.clone();
        issues.extend(self.switch_ui_missing.iter().cloned());
        issues.extend(self.corrupt.iter().cloned());
        issues
    }
}

pub fn audit_cache(resource_dir: &Path) -> CacheAudit {
    let mut report = CacheAudit::default();
    for (rel, members) in nier_pack::required_archive_members() {
        report.archives_total += 1;
        let file = resource_dir.join("data").join(&rel);
        if file.is_file() {report.archives_present += 1;}
        else {report.missing.push(format!("data/{rel}"));}
        for member in members {
            report.members_total += 1;
            let path = resource_dir.join("unpacked").join(&rel).join(&member);
            if path.is_file() {report.members_present += 1;}
            else {report.missing.push(format!("unpacked/{rel}/{member}"));}
        }
    }
    report.font_manifest = resource_dir.join("fonts/fonts.json").is_file();
    if !report.font_manifest {report.missing.push("fonts/fonts.json".into());}
    // All font families referenced by the MCD atlas manifest need glyph images.
    for id in ["00", "01", "02", "03", "04", "05", "08", "09", "10", "11"] {
        report.font_total += 1;
        let glyph_folder = resource_dir.join("fonts").join(id);
        let has_glyphs = glyph_folder.is_dir()
            && fs::read_dir(&glyph_folder).ok().is_some_and(|mut entries| entries.any(|item| {
                item.ok().is_some_and(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
            }));
        if has_glyphs {report.font_ready += 1;}
        else {report.missing.push(format!("fonts/{id}/*.png"));}
    }
    let ui_files = nier_pack::required_archive_members().keys()
        .filter(|name| name.starts_with("ui/") && name.ends_with(".dat"))
        .cloned().collect::<Vec<_>>();
    report.switch_ui_required = ui_files.len();
    let mut best_present = 0usize;
    for prefix in ["switch/data", "switch/data009", "switch_ui/data", "switch_ui/data009", "switch_ui_only/data", "switch_ui_only/data009", "switch_resources/data", "switch_resources/data009", "data_switch"] {
        let folder = resource_dir.join(prefix);
        if !folder.join("ui").is_dir() { continue; }
        let missing = ui_files.iter().flat_map(|dat| {
            let dtt=dat.replace(".dat", ".dtt");
            [dat.clone(), dtt]
        }).filter(|path| !folder.join(path).is_file()).collect::<Vec<_>>();
        let present = ui_files.len()*2 - missing.len();
        if present >= best_present {
            best_present=present;
            report.switch_ui_dat = ui_files.iter().filter(|p| folder.join(p.as_str()).is_file()).count();
            report.switch_ui_dtt = ui_files.iter().filter(|p| folder.join(p.replace(".dat", ".dtt")).is_file()).count();
            report.switch_ui_missing = missing.iter().map(|p|format!("{prefix}/{p}")).collect();
        }
    }
    if best_present == 0 {
        report.switch_ui_missing = ui_files.iter().flat_map(|file|[
            format!("switch/data009/{file}"),
            format!("switch/data009/{}",file.replace(".dat", ".dtt"))
        ]).collect();
    }
    report
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    #[test]
    fn empty_cache_reports_missing_originals() {
        let path = std::env::temp_dir().join(format!("nier-cache-test-{}", std::process::id()));
        assert!(!audit_cache(&path).missing.is_empty());
    }
}


pub fn verify_cache(resource_dir: &Path) -> CacheAudit {
    let mut report = audit_cache(resource_dir);
    for (rel, members) in nier_pack::required_archive_members() {
        if members.is_empty() { continue; }
        let path = resource_dir.join("data").join(&rel);
        let Ok(data) = fs::read(&path) else { continue; };
        match nier_pack::extract_dat_members(&data) {
            Ok(extracted) => {
                let lookup: BTreeMap<_, _> = extracted.into_iter().collect();
                for member in members {
                    let src = lookup.get(&member);
                    let cached = fs::read(resource_dir.join("unpacked").join(&rel).join(&member));
                    if let (Some(original), Ok(bytes)) = (src, cached) {
                        if *original == bytes { report.verified += 1; }
                        else { report.corrupt.push(format!("Mismatch: {rel}/{member}")); }
                    } else if src.is_none() {
                        report.corrupt.push(format!("Archive member absent: {rel}/{member}"));
                    }
                }
            }
            Err(error) => report.corrupt.push(format!("Invalid archive {rel}: {error}")),
        }
    }
    report
}
