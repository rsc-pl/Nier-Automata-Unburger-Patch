mod builder;
mod cpk;
mod nier_pack;
mod patch_zip;
mod project;
mod resources;
mod steam;
mod switch_subtitle_cpk;
mod translation;
mod ui;

use gtk4::prelude::*;

fn main() {
    // These commands work without opening the graphical interface.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && args[0] == "--switch-status" {
        match switch_subtitle_cpk::static_asset_status() {
            Ok(message) => { println!("{message}"); return; }
            Err(error) => { eprintln!("Switch packaging error: {error}"); std::process::exit(2); }
        }
    }
    if args.len() == 2 && args[0] == "--verify-switch-zip" {
        match patch_zip::verify_switch_zip(std::path::Path::new(&args[1])) {
            Ok(()) => { println!("Switch ZIP verified: {}", args[1]); return; }
            Err(error) => { eprintln!("Switch ZIP verification failed: {error}"); std::process::exit(2); }
        }
    }
    if !args.is_empty() {
        eprintln!("Unknown arguments. Supported: --switch-status | --verify-switch-zip <path>");
        std::process::exit(2);
    }
    let project = match project::Project::discover() {
        Ok(project) => project,
        Err(error) => {eprintln!("{error}");std::process::exit(1);}
    };
    if let Err(error)=project.init() {eprintln!("{error}");std::process::exit(1);}
    let settings=match project.load_settings() {
        Ok(value)=>value,
        Err(error)=>{eprintln!("{error}");std::process::exit(1);}
    };
    if !project.config_file().exists() {
        if let Err(error) = project.save_settings(&settings) {
            eprintln!("Unable to initialize project settings: {error}");
            std::process::exit(1);
        }
    }
    let app=gtk4::Application::builder().application_id("org.unburger.nierautomata").build();
    app.connect_activate(move |app| ui::launch(app,project.clone(),settings.clone()));
    app.run();
}
