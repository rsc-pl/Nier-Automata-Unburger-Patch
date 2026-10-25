mod translation;
use crate::{builder::{self,BuildEvent,BuildSettings}, project::{AppSettings,Project},resources,steam};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell,RefCell};
use std::fs;
use std::path::{Path,PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self,Receiver};
use std::time::Duration;

fn label(text: &str)->gtk::Label {
    let l=gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l
}
fn section(title:&str)->gtk::Label {
    let l=label(title);
    l.add_css_class("title-2");
    l
}
fn tab()->gtk::Box {
    let root=gtk::Box::new(gtk::Orientation::Vertical,12);
    root.set_margin_top(22);
    root.set_margin_bottom(22);
    root.set_margin_start(26);
    root.set_margin_end(26);
    root
}
fn line()->gtk::Separator{gtk::Separator::new(gtk::Orientation::Horizontal)}
fn field(label_text: &str, value:&str)->(gtk::Box,gtk::Entry){
    let box_=gtk::Box::new(gtk::Orientation::Vertical,5);
    box_.append(&label(label_text));
    let entry=gtk::Entry::new();
    entry.set_text(value);
    entry.set_hexpand(true);
    box_.append(&entry);
    (box_,entry)
}
fn scroll(child:&impl IsA<gtk::Widget>)->gtk::ScrolledWindow{
    let sc=gtk::ScrolledWindow::new();
    sc.set_hexpand(true);
    sc.set_vexpand(true);
    sc.set_child(Some(child));
    sc
}
fn append_log(log:&gtk::TextView,text:&str){
    let buffer=log.buffer();
    let mut iter=buffer.end_iter();
    buffer.insert(&mut iter,text);
}
fn save_settings(project:&Project,settings:&Rc<RefCell<AppSettings>>){
    if let Err(err)=project.save_settings(&settings.borrow()) {eprintln!("Settings could not be saved: {err}");}
}

pub fn launch(app:&gtk::Application,project:Project,initial:AppSettings){
    let window=gtk::ApplicationWindow::builder().application(app)
        .title("NieR:Automata UNBURGER").default_width(1160).default_height(770).build();
    let settings=Rc::new(RefCell::new(initial));
    let active=Rc::new(Cell::new(false));
    let inbox=Rc::new(RefCell::new(None::<Receiver<BuildEvent>>));
    let notebook=gtk::Notebook::new();
    notebook.set_hexpand(true);notebook.set_vexpand(true);
    let shell=gtk::Box::new(gtk::Orientation::Vertical,0);
    let header=gtk::Box::new(gtk::Orientation::Horizontal,12);
    header.set_margin_top(14);header.set_margin_bottom(14);header.set_margin_start(26);header.set_margin_end(26);
    let brand=label("NieR:Automata  /  UNBURGER");brand.add_css_class("title-2");brand.set_hexpand(true);
    header.append(&brand);
    let location=label("Project-local cache • Windows / Linux");location.add_css_class("dim-label");header.append(&location);
    shell.append(&header);shell.append(&line());shell.append(&notebook);

    // Build
    let build=tab();
    build.append(&section("Build game patches"));
    build.append(&label("Build PC and Switch translation ZIPs using original assets cached in .local/cache/resources. The Switch archive-loading compatibility investigation is ongoing."));
    let versions=label("Output will be written to .local/output/");versions.add_css_class("dim-label");build.append(&versions);
    let choice=gtk::Box::new(gtk::Orientation::Horizontal,16);
    let pc=gtk::CheckButton::with_label("PC ZIP");pc.set_active(settings.borrow().build_pc);
    let switch_=gtk::CheckButton::with_label("Switch ZIP");switch_.set_active(settings.borrow().build_switch);
    let fonts=gtk::CheckButton::with_label("Extra font DAT/DTT (optional)");fonts.set_active(settings.borrow().build_fonts);
    choice.append(&pc);choice.append(&switch_);choice.append(&fonts);build.append(&choice);
    let font_help=label("Switch UI DAT/DTT: automatic with Switch builds (native Switch sources required). The separate checkbox adds PC-derived font archives; it does not enable or disable Switch UI DAT/DTT.");
    font_help.add_css_class("dim-label");build.append(&font_help);
    let run=gtk::Button::with_label("Build patch");run.add_css_class("suggested-action");run.set_halign(gtk::Align::Start);build.append(&run);
    let progress=gtk::ProgressBar::new();progress.set_show_text(true);progress.set_text(Some("Ready"));build.append(&progress);
    build.append(&label("Build log"));
    let log=gtk::TextView::new();log.set_editable(false);log.set_cursor_visible(false);log.set_monospace(true);
    let log_scroll=scroll(&log);log_scroll.set_min_content_height(200);build.append(&log_scroll);
    notebook.append_page(&build,Some(&label("Build")));

    // Translation
    let translation_panel = translation::panel(&project, &settings);
    notebook.append_page(&translation_panel, Some(&label("Translation")));

    // Resources
    let resources_tab=tab();
    resources_tab.append(&section("Original game resources"));
    resources_tab.append(&label("Steam installations are detected automatically. You can also enter the game directory below. All game files remain outside the public repository."));
    let (game_box, game)=field("Game installation directory",&settings.borrow().game_dir);
    resources_tab.append(&game_box);
    let resource_actions=gtk::Box::new(gtk::Orientation::Horizontal,8);
    let auto=gtk::Button::with_label("Find Steam installation");
    let prepare=gtk::Button::with_label("Prepare source cache");prepare.add_css_class("suggested-action");
    resource_actions.append(&auto);resource_actions.append(&prepare);resources_tab.append(&resource_actions);
    let cache_heading=section("Prepared resource cache");
    resources_tab.append(&cache_heading);
    let resource_status=label(&resources::audit_cache(&project.resources()).status());
    resource_status.set_selectable(true);resource_status.set_wrap(true);
    resources_tab.append(&resource_status);
    let cache_refresh=gtk::Button::with_label("Verify cached resources");
    cache_refresh.set_halign(gtk::Align::Start);
    resources_tab.append(&cache_refresh);
    let cache_issues=gtk::TextView::new();
    cache_issues.set_editable(false);cache_issues.set_cursor_visible(false);
    cache_issues.set_monospace(true);
    cache_issues.set_wrap_mode(gtk::WrapMode::WordChar);
    let problems_scroll=scroll(&cache_issues);
    problems_scroll.set_min_content_height(125);
    resources_tab.append(&problems_scroll);
    resources_tab.append(&line());
    let browser = gtk::Expander::new(Some("Choose a game directory manually"));
    browser.set_expanded(false);
    let browser_content=gtk::Box::new(gtk::Orientation::Vertical,8);
    browser_content.append(&label("Browse game folders inside this window"));
    let start=if !settings.borrow().game_dir.is_empty() {PathBuf::from(&settings.borrow().game_dir)}
      else {std::env::var("HOME").map(PathBuf::from).or_else(|_|std::env::var("USERPROFILE").map(PathBuf::from)).unwrap_or_else(|_|PathBuf::from("."))};
    let folder=gtk::Entry::new();folder.set_text(&start.to_string_lossy());browser_content.append(&folder);
    let folder_actions=gtk::Box::new(gtk::Orientation::Horizontal,8);
    let folder_up=gtk::Button::with_label("Parent directory");
    let folder_use=gtk::Button::with_label("Use this directory");
    folder_actions.append(&folder_up);folder_actions.append(&folder_use);browser_content.append(&folder_actions);
    let directory_listing=gtk::ListBox::new();directory_listing.set_selection_mode(gtk::SelectionMode::Single);
    let dir_scroller=scroll(&directory_listing);
    dir_scroller.set_min_content_height(160);
    browser_content.append(&dir_scroller);
    browser.set_child(Some(&browser_content));resources_tab.append(&browser);
    let rscroll=scroll(&resources_tab);
    notebook.append_page(&rscroll,Some(&label("Resources")));

    // Settings
    let settings_tab=tab();
    settings_tab.append(&section("Project settings"));
    settings_tab.append(&label("All settings, caches, diagnostics and generated output are saved inside the project’s .local directory. No user-wide configuration is created."));
    let path_cfg=label(&format!("Config: {}",project.config_file().display()));path_cfg.set_selectable(true);
    settings_tab.append(&path_cfg);
    let path_cache=label(&format!("Cache: {}",project.resources().display()));path_cache.set_selectable(true);
    settings_tab.append(&path_cache);
    let path_output=label(&format!("Output: {}",project.output().display()));path_output.set_selectable(true);
    settings_tab.append(&path_output);
    settings_tab.append(&line());
    settings_tab.append(&section("Translation reference"));
    settings_tab.append(&label("Default: translations/JP. Leave empty to use that folder; relative paths resolve from the project root."));
    let (reference_box, reference_dir)=field("Reference directory (optional override)",&settings.borrow().reference_dir);
    settings_tab.append(&reference_box);
    settings_tab.append(&line());
    settings_tab.append(&section("Editor preferences"));
    let show_empty=gtk::CheckButton::with_label("Show empty translation entries");
    show_empty.set_active(settings.borrow().show_empty_rows);
    settings_tab.append(&show_empty);
    let limit_row=gtk::Box::new(gtk::Orientation::Horizontal,12);
    limit_row.append(&label("Per-line character warning (0 = disabled)"));
    let line_limit=gtk::SpinButton::with_range(0.0, 300.0, 1.0);
    line_limit.set_value(settings.borrow().line_warning_limit as f64);
    limit_row.append(&line_limit);settings_tab.append(&limit_row);
    let config_hint=label("Settings are saved to .local/config.toml. Translation preferences update automatically.");
    config_hint.add_css_class("dim-label");settings_tab.append(&config_hint);
    settings_tab.append(&line());
    settings_tab.append(&label("Preflight checks and build errors are displayed inside the Build and Resources tabs, without dialog windows."));
    notebook.append_page(&settings_tab,Some(&label("Settings")));

    // Callbacks
    let proj=project.clone();let cfg=settings.clone();
    game.connect_changed(move |e|{cfg.borrow_mut().game_dir=e.text().to_string();save_settings(&proj,&cfg);});
    for (button,platform) in [(pc.clone(),0),(switch_.clone(),1),(fonts.clone(),2)] {
        let proj=project.clone();let cfg=settings.clone();
        button.connect_toggled(move |b|{
            match platform {0=>cfg.borrow_mut().build_pc=b.is_active(),1=>cfg.borrow_mut().build_switch=b.is_active(),_=>cfg.borrow_mut().build_fonts=b.is_active()};
            save_settings(&proj,&cfg);
        });
    }
    let proj=project.clone();let cfg=settings.clone();
    reference_dir.connect_changed(move |field|{
        cfg.borrow_mut().reference_dir=field.text().to_string();
        save_settings(&proj,&cfg);
    });
    let proj=project.clone();let cfg=settings.clone();
    show_empty.connect_toggled(move |button|{
        cfg.borrow_mut().show_empty_rows=button.is_active();
        save_settings(&proj,&cfg);
    });
    let proj=project.clone();let cfg=settings.clone();
    line_limit.connect_value_changed(move |button|{
        cfg.borrow_mut().line_warning_limit=button.value() as u32;
        save_settings(&proj,&cfg);
    });
    let status=resource_status.clone();let issues=cache_issues.clone();let project_copy=project.clone();
    let refresh_cache=move || {
        let report=resources::audit_cache(&project_copy.resources());
        status.set_text(&report.status());
        let missing_resources=report.issues();
        let message=if missing_resources.is_empty() {"All indexed PC and Switch resources present.".to_string()} else {
            let mut msg=format!("{} missing or unprepared resources:\n",missing_resources.len());
            for missing in missing_resources.iter().take(250) {msg.push_str(missing);msg.push('\n');}
            if missing_resources.len()>250 {msg.push_str("… additional entries omitted");}
            msg
        };
        issues.buffer().set_text(&message);
    };
    refresh_cache();
    let resource_dir=project.resources();
    let verify_status=resource_status.clone();let verify_issues=cache_issues.clone();
    cache_refresh.connect_clicked(move |_|{
        let report=resources::verify_cache(&resource_dir);
        verify_status.set_text(&report.status());
        let issues=report.issues();
        verify_issues.buffer().set_text(&if issues.is_empty() {
            "All indexed archive members verified against original cached DAT contents.".into()
        } else {issues.join("\n")});
    });
    let project_copy=project.clone();let settings_copy=settings.clone();let active_copy=active.clone();let inbox_copy=inbox.clone();let log_copy=log.clone();let pc_copy=pc.clone();let switch_copy=switch_.clone();let fonts_copy=fonts.clone();
    run.connect_clicked(move |_|{
        if active_copy.get(){return;}
        if !pc_copy.is_active()&&!switch_copy.is_active(){append_log(&log_copy,"Choose at least one output platform.\n");return;}
        active_copy.set(true);
        log_copy.buffer().set_text("");
        let resources=project_copy.resources();
        let cfg=settings_copy.borrow().clone();
        let options=BuildSettings{resources_dir:Some(resources),translations_dir:Some(project_copy.translations()),output_dir:Some(project_copy.output()),build_fonts:fonts_copy.is_active(),build_pc_zip:pc_copy.is_active(),build_switch_zip:switch_copy.is_active()};
        let (tx,rx)=mpsc::channel();*inbox_copy.borrow_mut()=Some(rx);
        std::thread::spawn(move ||{
            let result=builder::run_nier_pack(&tx,&options,&cfg.patch_version);
            let _=tx.send(BuildEvent::Finished(result));
        });
    });
    let game_copy=game.clone();let status=resource_status.clone();
    auto.connect_clicked(move |_|{
        let paths=steam::installations();
        if let Some(first)=paths.first(){game_copy.set_text(&first.to_string_lossy());status.set_text(&format!("Steam: {} install(s) detected",paths.len()));}
        else {status.set_text("Steam game installation not found. Enter or browse its directory.");}
    });
    let status=resource_status.clone();let game_copy=game.clone();let project_copy=project.clone();let active_copy=active.clone();let inbox_copy=inbox.clone();
    prepare.connect_clicked(move |_|{
        if active_copy.get(){return;}
        let game_dir=PathBuf::from(game_copy.text().to_string());
        if !steam::valid_installation(&game_dir){status.set_text("Invalid game directory. Select a folder containing data/core/corehap.dat or data*.cpk.");return;}
        active_copy.set(true);
        status.set_text("Preparing source files and checking cache…");
        let dest=project_copy.resources();
        let (tx,rx)=mpsc::channel();*inbox_copy.borrow_mut()=Some(rx);
        std::thread::spawn(move ||{
            match resources::prepare_from_install(&game_dir,&dest){
                Ok(r)=>{
                    let _=tx.send(BuildEvent::Log(format!("Resources: {} archives prepared; {} members extracted; {} cache hits; {} missing sources.\n",r.archives_ready,r.members_ready,r.cache_hits,r.absent.len())));
                    for note in r.notes {let _=tx.send(BuildEvent::Log(format!("Note: {note}\n")));}
                    for name in r.absent.iter().take(25){let _=tx.send(BuildEvent::Log(format!("Missing archive: {name}\n")));}
                    let _=tx.send(BuildEvent::Finished(Ok(())));
                },
                Err(e)=>{let _=tx.send(BuildEvent::Finished(Err(e)));},
            }
        });
    });
    let directory_copy=directory_listing.clone();let folder_copy=folder.clone();
    let refresh=move |directory:&Path|{
        while let Some(child)=directory_copy.first_child(){directory_copy.remove(&child);}
        if let Ok(entries)=fs::read_dir(directory){
            let mut folders=entries.filter_map(|entry|entry.ok().map(|e|e.path())).filter(|p|p.is_dir()).collect::<Vec<_>>();
            folders.sort();
            for p in folders.into_iter().take(300){
                let row=gtk::ListBoxRow::new();row.set_widget_name(&p.to_string_lossy());
                let item=label(&p.file_name().unwrap_or_default().to_string_lossy());
                item.set_margin_start(12);item.set_margin_top(5);item.set_margin_bottom(5);
                row.set_child(Some(&item));directory_copy.append(&row);
            }
        }
    };
    refresh(&PathBuf::from(folder.text().to_string()));
    let refresh=Rc::new(refresh);
    let folders=folder.clone();let refresh_copy=refresh.clone();
    directory_listing.connect_row_activated(move |_,row|{
        let path=PathBuf::from(row.widget_name().to_string());
        folders.set_text(&path.to_string_lossy());refresh_copy(&path);
    });
    let folders=folder.clone();let refresh_copy=refresh.clone();
    folder_up.connect_clicked(move |_|{
        let path=PathBuf::from(folders.text().to_string());
        if let Some(parent)=path.parent(){folders.set_text(&parent.to_string_lossy());refresh_copy(parent);}
    });
    let folders=folder.clone();let game_copy=game.clone();
    folder_use.connect_clicked(move |_|game_copy.set_text(&folders.text()));
    let folders=folder.clone();let refresh_copy=refresh.clone();
    folder.connect_activate(move |_|refresh_copy(&PathBuf::from(folders.text().to_string())));

    let rx=inbox.clone();let running=active.clone();let build_log=log.clone();let pg=progress.clone();
    let status=resource_status.clone();let button=run.clone();let preparer=prepare.clone();
    let cache_report=cache_issues.clone();let project_copy=project.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(80),move ||{
        let events={
            let channel=rx.borrow();
            if let Some(channel)=channel.as_ref(){
                let mut messages=Vec::new();
                while let Ok(event)=channel.try_recv(){messages.push(event);}
                messages
            } else {Vec::new()}
        };
        for event in events {
            match event {
                BuildEvent::Log(line)=>{append_log(&build_log,&line);},
                BuildEvent::Progress(now,total)=>{pg.set_fraction(if total==0 {0.0} else {now as f64/total as f64});pg.set_text(Some(&format!("{now}/{total}")));},
                BuildEvent::Finished(result)=>{
                    running.set(false);
                    match result{Ok(())=>{append_log(&build_log,"Completed.\n");status.set_text("Completed");},Err(e)=>{append_log(&build_log,&format!("Error: {e}\n"));status.set_text(&format!("Error: {e}"));}}
                    let audit=resources::audit_cache(&project_copy.resources());
                    status.set_text(&audit.status());
                    cache_report.buffer().set_text(&audit.issues().into_iter().take(250).collect::<Vec<_>>().join("\n"));
                    *rx.borrow_mut()=None;
                },
            }
        }
        button.set_sensitive(!running.get());preparer.set_sensitive(!running.get());
        gtk::glib::ControlFlow::Continue
    });
    window.set_child(Some(&shell));
    window.present();
}
