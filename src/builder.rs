use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct BuildSettings {
    pub resources_dir: Option<PathBuf>,
    pub translations_dir: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub build_fonts: bool,
    pub build_pc_zip: bool,
    pub build_switch_zip: bool,
}

#[derive(Debug, Clone)]
pub enum BuildEvent {
    Log(String),
    Progress(u32, u32),
    Finished(Result<(), String>),
}

pub fn run_nier_pack(
    sender: &std::sync::mpsc::Sender<BuildEvent>,
    settings: &BuildSettings,
    patch_version: &str,
) -> Result<(), String> {
    let resources = settings
        .resources_dir
        .as_deref()
        .ok_or_else(|| "Config resources directory is not selected.".to_string())?;
    let translations = settings
        .translations_dir
        .as_deref()
        .ok_or_else(|| "Translated properties directory is not selected.".to_string())?;
    let output = settings
        .output_dir
        .as_deref()
        .ok_or_else(|| "Output directory is not selected.".to_string())?;

    require_dir(&resources.join("data"), "resources data/")?;
    require_dir(&resources.join("unpacked"), "resources unpacked/")?;
    require_dir(&resources.join("fonts"), "resources fonts/")?;
    require_dir(translations, "translated properties directory")?;

    std::fs::create_dir_all(output)
        .map_err(|e| format!("Failed to create {}: {e}", output.display()))?;
    let build_output = output.join("_nier_built_files");
    let _ = std::fs::remove_dir_all(&build_output);
    std::fs::create_dir_all(&build_output)
        .map_err(|e| format!("Failed to create {}: {e}", build_output.display()))?;

    let _ = sender.send(BuildEvent::Progress(0, 4));
    let _ = sender.send(BuildEvent::Log(format!(
        "Using Rust Nier packer for DATs and UI MCDs.\nResources: {}\nTranslations: {}\nOutput: {}\n",
        resources.display(),
        translations.display(),
        output.display()
    )));

    let report = crate::nier_pack::build_non_ui(&crate::nier_pack::NierPackOptions {
        resources_dir: resources.to_path_buf(),
        translations_dir: translations.to_path_buf(),
        output_dir: build_output.clone(),
        include_ui_mcd: settings.build_pc_zip,
    })?;
    let _ = sender.send(BuildEvent::Log(format!(
        "Build jobs: {} total, {} built, {} UI/MCD skipped, {} archives written ({} UI texture archives).\n",
        report.jobs_total,
        report.jobs_built,
        report.jobs_skipped_mcd,
        report.archives_written,
        report.ui_texture_archives_written
    )));
    if !report.missing_translations.is_empty() {
        let _ = sender.send(BuildEvent::Log(format!(
            "Missing translated files skipped: {}\n",
            report.missing_translations.len()
        )));
        for path in report.missing_translations.iter().take(20) {
            let _ = sender.send(BuildEvent::Log(format!("- {path}\n")));
        }
        if report.missing_translations.len() > 20 {
            let _ = sender.send(BuildEvent::Log("...more omitted\n".into()));
        }
    }
    let _ = sender.send(BuildEvent::Progress(1, 4));

    if settings.build_fonts {
        if let Err(err) = require_dir(&resources.join("fonts"), "resources fonts/").and_then(|_| {
            let _ = sender.send(BuildEvent::Log(
                "Building optional Nier font DAT/DTT files with Rust table patching and ImageMagick DDS conversion.\n"
                    .into(),
            ));
            crate::nier_pack::build_fonts(
                &crate::nier_pack::NierPackOptions {
                    resources_dir: resources.to_path_buf(),
                    translations_dir: translations.to_path_buf(),
                    output_dir: build_output.clone(),
                    include_ui_mcd: false,
                },
                "magick",
            )
            .map(|font_report| {
                let _ = sender.send(BuildEvent::Log(format!(
                    "Built {} font families.\n",
                    font_report.fonts_built
                )));
            })
        }) {
            let _ = sender.send(BuildEvent::Log(format!(
                "Optional font DAT build skipped: {err}\n"
            )));
        }
    }
    let _ = sender.send(BuildEvent::Progress(2, 4));

    let version_suffix = version_suffix(patch_version);
    if settings.build_pc_zip {
        let zip = output.join(format!("Nier-Automata-Unburger-PC{version_suffix}.zip"));
        crate::patch_zip::write_patch_zip(&build_output, &zip, Some("data"), Some(patch_version))?;
        let _ = sender.send(BuildEvent::Log(format!(
            "Created PC zip: {}\n",
            zip.display()
        )));
    }

    let mut switch_ui_output = None;
    if settings.build_switch_zip {
        let switch_ui_dir = output.join("_nier_switch_ui_files");
        let _ = std::fs::remove_dir_all(&switch_ui_dir);
        let switch_report = crate::nier_pack::build_switch_ui_text(&crate::nier_pack::NierPackOptions {
            resources_dir: resources.to_path_buf(),
            translations_dir: translations.to_path_buf(),
            output_dir: switch_ui_dir.clone(),
            include_ui_mcd: false,
        })?;
        if let Some(data_dir) = &switch_report.switch_data_dir {
            let _ = sender.send(BuildEvent::Log(format!(
                "Switch UI source: {}\nSwitch UI jobs: {} total, {} built, {} DAT archives written, {} DTT archives copied.\n",
                data_dir.display(),
                switch_report.jobs_total,
                switch_report.jobs_built,
                switch_report.dat_archives_written,
                switch_report.dtt_archives_copied
            )));
            if !switch_report.missing_translations.is_empty() {
                let _ = sender.send(BuildEvent::Log(format!(
                    "Missing Switch UI translated files skipped: {}\n",
                    switch_report.missing_translations.len()
                )));
            }
            if !switch_report.missing_sources.is_empty() {
                let _ = sender.send(BuildEvent::Log(format!(
                    "Missing Switch UI source files skipped: {}\n",
                    switch_report.missing_sources.len()
                )));
                for path in switch_report.missing_sources.iter().take(20) {
                    let _ = sender.send(BuildEvent::Log(format!("- {path}\n")));
                }
                if switch_report.missing_sources.len() > 20 {
                    let _ = sender.send(BuildEvent::Log("...more omitted\n".into()));
                }
            }
            if switch_report.jobs_built > 0 {
                switch_ui_output = Some(switch_ui_dir);
            }
        } else {
            let _ = sender.send(BuildEvent::Log(
                "Switch UI source not found; Switch zip will exclude PC-built ui/ files to avoid incompatible UI archives.\n"
                    .into(),
            ));
        }
    }

    if settings.build_switch_zip {
        // Switch subtitle files are packaged in data300.cpk to preserve
        // cinematic playback. All other translated resources remain loose.
        let switch_stage = output.join("_nier_switch_stage");
        let _ = std::fs::remove_dir_all(&switch_stage);
        let switch_romfs = switch_stage.join("romfs");
        copy_dir_filtered(&build_output, &switch_romfs, true, true)?;
        if let Some(switch_ui_dir) = &switch_ui_output {
            copy_dir_filtered(switch_ui_dir, &switch_romfs, false, true)?;
        }
        let subtitle_count = crate::switch_subtitle_cpk::build(
            &build_output.join("subtitle"), &switch_romfs.join("data300.cpk"),
        )?;
        let ips = crate::switch_subtitle_cpk::write_update_102_ips(&switch_stage.join("exefs"))?;
        let _ = sender.send(BuildEvent::Log(format!(
            "Switch: {subtitle_count} subtitles packed into romfs/data300.cpk; no loose subtitle/ files.\nTarget update: 1.0.2 Build ID {}\nIPS: {}\n",
            crate::switch_subtitle_cpk::UPDATE_102_BUILD_ID,
            ips.display(),
        )));
        let zip = output.join(format!("Nier-Automata-Unburger-Switch{version_suffix}.zip"));
        // Remove a previous output before building, so failed builds cannot
        // leave an outdated archive that looks newly generated.
        if zip.exists() {
            std::fs::remove_file(&zip)
                .map_err(|e| format!("Cannot remove stale Switch ZIP {}: {e}", zip.display()))?;
        }
        let pending = output.join(format!("Nier-Automata-Unburger-Switch{version_suffix}.zip.partial"));
        if pending.exists() {
            std::fs::remove_file(&pending)
                .map_err(|e| format!("Cannot remove stale partial ZIP {}: {e}", pending.display()))?;
        }
        crate::patch_zip::write_patch_zip(
            &switch_stage,
            &pending,
            Some("Nier Automata Unburger"),
            Some(patch_version),
        )?;
        crate::patch_zip::verify_switch_zip(&pending)?;
        std::fs::rename(&pending, &zip)
            .map_err(|e| format!("Cannot finalize Switch ZIP {}: {e}", zip.display()))?;
        let _ = sender.send(BuildEvent::Log(format!(
            "Switch ZIP verified: {}\nVerified: data300.cpk, update 1.0.2 IPS, and zero loose subtitle files.\n",
            zip.display()
        )));
    }
    let _ = sender.send(BuildEvent::Progress(3, 4));

    let _ = std::fs::remove_dir_all(switch_stage_path(output));
    let _ = std::fs::remove_dir_all(output.join("_nier_switch_ui_files"));
    let _ = sender.send(BuildEvent::Progress(4, 4));
    Ok(())
}

fn require_dir(path: &Path, label: &str) -> Result<(), String> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(format!("Missing {label}: {}", path.display()))
    }
}

fn switch_stage_path(output: &Path) -> PathBuf {
    output.join("_nier_switch_stage")
}

fn copy_dir_filtered(source: &Path, target: &Path, skip_top_level_ui: bool, skip_top_level_subtitle: bool) -> Result<(), String> {
    std::fs::create_dir_all(target)
        .map_err(|e| format!("Failed to create {}: {e}", target.display()))?;
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry.map_err(|e| format!("Failed to walk {}: {e}", source.display()))?;
        let path = entry.path();
        if path == source {
            continue;
        }
        let rel = path
            .strip_prefix(source)
            .map_err(|e| format!("Failed to build relative path for {}: {e}", path.display()))?;
        if rel.components().any(|component| {
            component
                .as_os_str()
                .to_string_lossy()
                .starts_with("_nier_")
        }) {
            continue;
        }
        if (skip_top_level_ui
            && rel.components().next().is_some_and(|c| c.as_os_str() == "ui"))
            || (skip_top_level_subtitle
                && rel.components().next().is_some_and(|c| c.as_os_str() == "subtitle"))
        {
            continue;
        }
        let dest = target.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest)
                .map_err(|e| format!("Failed to create {}: {e}", dest.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
            }
            std::fs::copy(path, &dest).map_err(|e| {
                format!(
                    "Failed to copy {} to {}: {e}",
                    path.display(),
                    dest.display()
                )
            })?;
        }
    }
    Ok(())
}

fn version_suffix(patch_version: &str) -> String {
    if patch_version.trim().is_empty() {
        String::new()
    } else {
        format!("-{}", patch_version.trim().replace(' ', "-"))
    }
}

