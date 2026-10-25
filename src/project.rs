use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub game_dir: String,
    pub patch_version: String,
    pub build_pc: bool,
    pub build_switch: bool,
    pub build_fonts: bool,
    pub last_translation: String,
    pub reference_dir: String,
    pub show_empty_rows: bool,
    pub line_warning_limit: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            game_dir: String::new(),
            patch_version: "0.6.0".into(),
            build_pc: true,
            build_switch: true,
            build_fonts: false,
            last_translation: String::new(),
            reference_dir: "translations/JP".into(),
            show_empty_rows: true,
            line_warning_limit: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
}

impl Project {
    pub fn discover() -> Result<Self, String> {
        // Resolve against the executable and current folder, never a user-specific path.
        let exe_dir = std::env::current_exe().map_err(|e| e.to_string())?
            .parent().ok_or("Unable to locate executable folder")?.to_path_buf();
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        for origin in [exe_dir, cwd] {
            for candidate in origin.ancestors().take(7) {
                if candidate.join("Cargo.toml").is_file()
                    && candidate.join("translations").is_dir()
                    && candidate.join("nier-pack/build_localization.sh").is_file()
                {
                    return Ok(Self { root: candidate.to_path_buf() });
                }
            }
        }
        Err("Project directory not found. Run the executable from the project checkout.".into())
    }

    pub fn local(&self) -> PathBuf { self.root.join(".local") }
    pub fn resources(&self) -> PathBuf { self.local().join("cache").join("resources") }
    pub fn output(&self) -> PathBuf { self.local().join("output") }
    pub fn translations(&self) -> PathBuf { self.root.join("translations") }
    pub fn reference_dir(&self, configured: &str) -> PathBuf {
        let value = configured.trim();
        if value.is_empty() { return self.translations().join("JP"); }
        let path = PathBuf::from(value);
        if path.is_absolute() { path } else { self.root.join(path) }
    }
    pub fn config_file(&self) -> PathBuf { self.local().join("config.toml") }
    pub fn init(&self) -> Result<(), String> {
        for path in [self.resources().join("data"), self.resources().join("unpacked"),
            self.resources().join("fonts"), self.output(), self.local().join("docs"), self.local().join("reports")]
        {
            fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }
    pub fn load_settings(&self) -> Result<AppSettings, String> {
        let p = self.config_file();
        if !p.is_file() { return Ok(AppSettings::default()); }
        toml::from_str(&fs::read_to_string(&p).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid {}: {e}", p.display()))
    }
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        let p = self.config_file();
        fs::create_dir_all(p.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
        let value = toml::to_string_pretty(settings).map_err(|e| e.to_string())?;
        let temp = p.with_extension("toml.new");
        fs::write(&temp, value).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        if p.exists() { fs::remove_file(&p).map_err(|e| e.to_string())?; }
        fs::rename(&temp, &p).map_err(|e| e.to_string())
    }
}
