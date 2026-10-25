use crate::project::{AppSettings, Project};
use crate::translation::TranslationDocument;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn label(value: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(value));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn list_scroll(widget: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let sc = gtk::ScrolledWindow::new();
    sc.set_hexpand(true);
    sc.set_vexpand(true);
    sc.set_child(Some(widget));
    sc
}

fn text(buffer: &gtk::TextBuffer) -> String {
    buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string()
}

#[derive(Default)]
struct EditorState {
    path: Option<PathBuf>,
    doc: Option<TranslationDocument>,
    selected: Option<usize>,
    reference: Option<TranslationDocument>,
}

fn files(root: &Path) -> Vec<String> {
    let mut names = fs::read_dir(root).ok().into_iter().flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|f| f.ends_with(".properties") || f.ends_with("_eng.txt"))
        .collect::<Vec<_>>();
    let priority = fs::read_to_string(root.parent().unwrap_or(root).join("nier_automata_files_order.txt"))
        .unwrap_or_default();
    let indices = priority.lines().map(str::trim).enumerate()
        .map(|(n, f)| (f.to_string(), n)).collect::<std::collections::HashMap<_, _>>();
    names.sort_by(|a, b| {
        indices.get(a).copied().unwrap_or(usize::MAX)
            .cmp(&indices.get(b).copied().unwrap_or(usize::MAX))
            .then(a.cmp(b))
    });
    names
}

fn update_counter(label: &gtk::Label, value: &str, limit: u32) {
    let lines = value.split('\n').collect::<Vec<_>>();
    let lengths = lines.iter().map(|line| line.trim_end_matches('\r').chars().count()).collect::<Vec<_>>();
    let exceeded = limit != 0 && lengths.iter().any(|count| *count > limit as usize);
    let suffix = if limit == 0 { String::new() }
        else { format!(" • limit {limit}/line") };
    label.set_text(&format!("{} characters • {} lines • [{}]{}",
        value.chars().count(), lines.len(),
        lengths.iter().map(usize::to_string).collect::<Vec<_>>().join(" / "), suffix));
    if exceeded {label.add_css_class("error");} else {label.remove_css_class("error");}
}

fn repaint_lines(view: &gtk::TextView) {
    let buffer = view.buffer();
    let table = buffer.tag_table();
    for (name, color) in [("controls", "#C084FC"), ("brackets", "#60A5FA")] {
        if table.lookup(name).is_none() {
            let tag = gtk::TextTag::new(Some(name));
            tag.set_property("foreground", color);
            table.add(&tag);
        }
    }
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    for name in ["controls", "brackets"] {
        if let Some(tag) = table.lookup(name) { buffer.remove_tag(&tag, &start, &end); }
    }
    let value = text(&buffer);
    // Highlight inline codes without changing any stored characters.
    let chars = value.chars().collect::<Vec<_>>();
    let mut at = 0usize;
    while at < chars.len() {
        let bracket = matches!(chars[at], '[' | '<' | '{');
        let end_ch = match chars[at] { '[' => ']', '<' => '>', '{' => '}', _ => '\0' };
        let code = chars[at] == '%' && at + 1 < chars.len() && chars[at + 1].is_ascii_alphabetic();
        let finish = if bracket {
            chars.iter().enumerate().skip(at + 1).find(|(_, c)| **c == end_ch)
                .map(|(i, _)| i + 1).filter(|i| i - at < 150)
        } else if code { Some(at + 2) } else { None };
        if let Some(to) = finish {
            let key = if bracket { "brackets" } else { "controls" };
            if let Some(tag) = table.lookup(key) {
                let a = buffer.iter_at_offset(at as i32);
                let b = buffer.iter_at_offset(to as i32);
                buffer.apply_tag(&tag, &a, &b);
            }
            at = to;
        } else { at += 1; }
    }
}

fn draw_rows(rows: &gtk::ListBox, state: &Rc<RefCell<EditorState>>, filter: &str, show_empty: bool) {
    while let Some(item) = rows.first_child() { rows.remove(&item); }
    let model = state.borrow();
    let Some(doc) = model.doc.as_ref() else { return; };
    let query = filter.to_lowercase();
    for (i, row) in doc.rows.iter().enumerate() {
        if !show_empty && row.value.is_empty() { continue; }
        if !query.is_empty() && !row.key.to_lowercase().contains(&query)
            && !row.value.to_lowercase().contains(&query) { continue; }
        let item = gtk::ListBoxRow::new();
        item.set_widget_name(&i.to_string());
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 3);
        outer.set_margin_start(9); outer.set_margin_end(9);
        outer.set_margin_top(5); outer.set_margin_bottom(6);
        let id = label(&row.key);
        id.add_css_class("caption");
        if row.dirty { id.add_css_class("accent"); }
        let preview = label(&row.value.replace('\r', "").replace('\n', " ↵ "));
        preview.set_max_width_chars(45);
        if row.value.is_empty() { preview.add_css_class("dim-label"); }
        outer.append(&id); outer.append(&preview);
        item.set_child(Some(&outer));
        rows.append(&item);
    }
}

pub fn panel(project: &Project, settings: &Rc<RefCell<AppSettings>>) -> gtk::Box {
    let main = gtk::Box::new(gtk::Orientation::Vertical, 10);
    main.set_margin_top(18); main.set_margin_bottom(14);
    main.set_margin_start(18); main.set_margin_end(18);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 15);
    let title = label("Translation editor");
    title.add_css_class("title-2"); title.set_hexpand(true);
    top.append(&title);
    top.append(&label("Patch version:"));
    let version = gtk::Entry::new();
    version.set_width_chars(12);
    version.set_text(&settings.borrow().patch_version);
    top.append(&version);
    main.append(&top);
    let caption = label("Parsed keys and decoded values • search in keys and translations • unmodified entries remain byte-identical");
    caption.add_css_class("dim-label");main.append(&caption);

    let horizontal = gtk::Paned::new(gtk::Orientation::Horizontal);
    horizontal.set_wide_handle(true);
    horizontal.set_vexpand(true);
    horizontal.set_position(265);
    let file_col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let file_filter = gtk::SearchEntry::new();
    file_filter.set_placeholder_text(Some("Find translation file"));
    file_col.append(&file_filter);
    let file_count = label("Files");file_col.append(&file_count);
    let file_list = gtk::ListBox::new();
    file_list.set_selection_mode(gtk::SelectionMode::Single);
    file_col.append(&list_scroll(&file_list));
    horizontal.set_start_child(Some(&file_col));

    let detail_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
    detail_pane.set_wide_handle(true);
    detail_pane.set_position(380);
    let record_col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let record_filter = gtk::SearchEntry::new();
    record_filter.set_placeholder_text(Some("Search keys and text"));
    record_col.append(&record_filter);
    let row_summary = label("Select a file");record_col.append(&row_summary);
    let row_list = gtk::ListBox::new();
    row_list.set_selection_mode(gtk::SelectionMode::Single);
    record_col.append(&list_scroll(&row_list));
    detail_pane.set_start_child(Some(&record_col));

    let edit_col = gtk::Box::new(gtk::Orientation::Vertical, 9);
    edit_col.set_margin_start(12);
    let key_label = label("Select a record");
    key_label.set_selectable(true);
    key_label.add_css_class("heading");
    edit_col.append(&key_label);
    let original_caption = label("Original / reference text (read-only)");
    original_caption.add_css_class("dim-label");edit_col.append(&original_caption);
    let original_text = gtk::TextView::new();
    original_text.set_monospace(true);
    original_text.set_editable(false);
    original_text.set_cursor_visible(false);
    original_text.set_wrap_mode(gtk::WrapMode::WordChar);
    original_text.set_left_margin(12);
    original_text.set_top_margin(8);
    let reference_scroll = list_scroll(&original_text);
    reference_scroll.set_min_content_height(110);
    edit_col.append(&reference_scroll);
    let value_label = label("Translation — decoded multiline text with syntax highlighting");
    value_label.add_css_class("dim-label");edit_col.append(&value_label);
    let editor = gtk::TextView::new();
    editor.set_monospace(true);
    editor.set_wrap_mode(gtk::WrapMode::WordChar);
    editor.set_left_margin(12);editor.set_right_margin(12);
    editor.set_top_margin(10);editor.set_bottom_margin(10);
    editor.set_editable(false);
    edit_col.append(&list_scroll(&editor));
    let counter = label("0 chars • 0 lines");edit_col.append(&counter);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let save = gtk::Button::with_label("Save file");
    save.add_css_class("suggested-action");
    save.set_sensitive(false);
    let revert = gtk::Button::with_label("Discard edits");
    revert.set_sensitive(false);
    actions.append(&save);actions.append(&revert);edit_col.append(&actions);
    let status = label("Select a file, then a translation row.");
    status.set_wrap(true);
    status.set_selectable(true);
    edit_col.append(&status);
    detail_pane.set_end_child(Some(&edit_col));
    horizontal.set_end_child(Some(&detail_pane));
    main.append(&horizontal);

    let root = project.translations();
    let filenames = Rc::new(files(&root));
    let state = Rc::new(RefCell::new(EditorState::default()));
    let loading = Rc::new(Cell::new(false));
    let show_empty = Rc::new(Cell::new(settings.borrow().show_empty_rows));

    let fill_files: Rc<dyn Fn(&str)> = {
        let filenames = filenames.clone();let file_list = file_list.clone();let count = file_count.clone();
        Rc::new(move |q: &str| {
            while let Some(child) = file_list.first_child() { file_list.remove(&child); }
            let mut n = 0;
            for name in filenames.iter().filter(|name| name.to_lowercase().contains(&q.to_lowercase())) {
                let row = gtk::ListBoxRow::new(); row.set_widget_name(name);
                let cell = label(name);cell.set_margin_start(8);cell.set_margin_top(6);cell.set_margin_bottom(6);
                row.set_child(Some(&cell));file_list.append(&row);n += 1;
            }
            count.set_text(&format!("{n} of {} files", filenames.len()));
        })
    };
    fill_files("");
    let fill = fill_files.clone();
    file_filter.connect_search_changed(move |field| fill(&field.text()));

    let cfg = settings.clone();let project_copy = project.clone();
    version.connect_changed(move |field| {
        cfg.borrow_mut().patch_version = field.text().to_string();
        let _ = project_copy.save_settings(&cfg.borrow());
    });

    let state_copy = state.clone();let current_filter = record_filter.clone();
    let rows_copy = row_list.clone();let summary = row_summary.clone();let heading = key_label.clone();
    let output = editor.clone();let wait = loading.clone();let status_copy = status.clone();
    let save_copy = save.clone();let revert_copy = revert.clone();
    let root_copy = root.clone();let empty = show_empty.clone();
    let original_copy = original_text.clone();let original_caption_copy = original_caption.clone();
    let project_refs = project.clone();let cfg_ref=settings.clone();
    file_list.set_activate_on_single_click(true);
    file_list.connect_row_activated(move |_, selected| {
        let file = selected.widget_name().to_string();
        if state_copy.borrow().doc.as_ref().is_some_and(|doc| doc.dirty()) {
            status_copy.set_text("Unsaved changes: save or discard them before opening another file.");
            return;
        }
        let path = root_copy.join(&file);
        let result = if file.ends_with(".properties") {
            TranslationDocument::open(&path)
        } else { fs::read_to_string(&path).map(TranslationDocument::plain).map_err(|e| e.to_string()) };
        match result {
            Ok(doc) => {
                let count = doc.rows.len();
                let refs=project_refs.reference_dir(&cfg_ref.borrow().reference_dir);
                let reference_path=refs.join(&file);
                let reference=if reference_path.is_file() {
                    if file.ends_with(".properties") {
                        TranslationDocument::open(&reference_path).ok()
                    } else {
                        fs::read_to_string(&reference_path).ok().map(TranslationDocument::plain)
                    }
                } else {None};
                let ref_found=reference.is_some();
                *state_copy.borrow_mut() = EditorState { path: Some(path), doc: Some(doc), selected: None, reference };
                original_copy.buffer().set_text("");
                original_caption_copy.set_text(if ref_found {
                    "Japanese reference (read-only)"
                } else {
                    "Japanese reference not found in translations/JP (or configured reference directory)"
                });
                current_filter.set_text("");
                draw_rows(&rows_copy, &state_copy, "", empty.get());
                summary.set_text(&format!("{file} — {count} records"));
                heading.set_text("Select a record");
                wait.set(true);output.buffer().set_text("");wait.set(false);
                output.set_editable(false);
                save_copy.set_sensitive(false);revert_copy.set_sensitive(false);
                status_copy.set_text("Ready. Select a record to edit its decoded value.");
            }
            Err(e) => status_copy.set_text(&format!("Failed to parse {file}: {e}")),
        }
    });

    let state_copy = state.clone();let editor_copy = editor.clone();let title_copy = key_label.clone();
    let counter_copy = counter.clone();let loading_copy = loading.clone();let cfg = settings.clone();
    let original_copy = original_text.clone();
    row_list.connect_row_selected(move |_, selected| {
        let Some(row) = selected else { return; };
        let Ok(index) = row.widget_name().parse::<usize>() else { return; };
        let mut state = state_copy.borrow_mut();
        let Some(doc) = state.doc.as_ref() else { return; };
        let Some(item) = doc.rows.get(index) else { return; };
        let key = item.key.clone();let value = item.value.clone();
        let reference = state.reference.as_ref().and_then(|reference| reference.rows.iter().find(|row| row.key == key)).map(|row| row.value.clone());
        state.selected = Some(index);
        drop(state);
        title_copy.set_text(&key);
        original_copy.buffer().set_text(reference.as_deref().unwrap_or("No matching reference for this record."));
        repaint_lines(&original_copy);
        loading_copy.set(true);
        editor_copy.buffer().set_text(&value);
        editor_copy.set_editable(true);
        loading_copy.set(false);
        repaint_lines(&editor_copy);
        update_counter(&counter_copy, &value, cfg.borrow().line_warning_limit);
    });

    let state_copy = state.clone();let loading_copy = loading.clone();let edited_counter = counter.clone();
    let edited_save = save.clone();let edited_revert = revert.clone();let status_copy = status.clone();
    let editor_copy = editor.clone();let cfg = settings.clone();
    editor.buffer().connect_changed(move |buffer| {
        if loading_copy.get() {return;}
        let index = {
            let s = state_copy.borrow();
            let Some(index) = s.selected else { return; };
            if !s.doc.as_ref().is_some_and(|doc| doc.rows.get(index).is_some()) {return;}
            index
        };
        let value = text(buffer);
        if let Some(ref mut active) = state_copy.borrow_mut().doc {
            if active.rows[index].value != value { let _ = active.update(index, value.clone()); }
            edited_save.set_sensitive(active.dirty());
            edited_revert.set_sensitive(active.dirty());
        }
        update_counter(&edited_counter, &value, cfg.borrow().line_warning_limit);
        status_copy.set_text("Modified • not saved");
        repaint_lines(&editor_copy);
    });

    let state_copy = state.clone();let status_copy = status.clone();let save_copy = save.clone();
    let revert_copy = revert.clone();let rows_copy = row_list.clone();let row_filter_copy = record_filter.clone();let empty = show_empty.clone();
    save.connect_clicked(move |_| {
        let mut current = state_copy.borrow_mut();
        let Some(path) = current.path.clone() else { return; };
        let Some(doc) = current.doc.as_mut() else { return; };
        match doc.save(&path) {
            Ok(()) => {
                status_copy.set_text("Saved and validated");
                save_copy.set_sensitive(false);revert_copy.set_sensitive(false);
                drop(current);
                draw_rows(&rows_copy, &state_copy, &row_filter_copy.text(), empty.get());
            }
            Err(e) => status_copy.set_text(&format!("Save failed: {e}")),
        }
    });
    let state_copy = state.clone();let status_copy = status.clone();let save_copy = save.clone();
    let revert_copy = revert.clone();let editor_copy = editor.clone();let wait = loading.clone();
    let rows_copy = row_list.clone();let row_filter_copy = record_filter.clone();let empty = show_empty.clone();
    revert.connect_clicked(move |_| {
        let path = state_copy.borrow().path.clone();
        let Some(path) = path else { return; };
        let result = if path.extension().is_some_and(|ext| ext == "properties") {
            TranslationDocument::open(&path)
        } else {fs::read_to_string(&path).map(TranslationDocument::plain).map_err(|e| e.to_string())};
        match result {
            Ok(doc) => {
                let refs=state_copy.borrow().reference.clone();
                *state_copy.borrow_mut() = EditorState { path: Some(path), doc: Some(doc), selected: None, reference: refs };
                wait.set(true);editor_copy.buffer().set_text("");wait.set(false);
                editor_copy.set_editable(false);
                save_copy.set_sensitive(false);revert_copy.set_sensitive(false);
                draw_rows(&rows_copy, &state_copy, &row_filter_copy.text(), empty.get());
                status_copy.set_text("Changes discarded");
            }
            Err(e) => status_copy.set_text(&format!("Unable to reload: {e}")),
        }
    });
    let state_copy = state.clone();let rows_copy = row_list.clone();let empty = show_empty.clone();
    record_filter.connect_search_changed(move |field| draw_rows(&rows_copy, &state_copy, &field.text(), empty.get()));

    let empty_current=show_empty.clone();
    let prefs = settings.clone();let list=row_list.clone();let doc=state.clone();
    let row_query=record_filter.clone();let counter_copy=counter.clone();let editor_copy=editor.clone();
    let previous_limit=Rc::new(Cell::new(settings.borrow().line_warning_limit));
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(350),move || {
        let visible=prefs.borrow().show_empty_rows;
        if empty_current.get()!=visible {
            empty_current.set(visible);
            draw_rows(&list,&doc,&row_query.text(),visible);
        }
        let limit=prefs.borrow().line_warning_limit;
        if previous_limit.get()!=limit {
            previous_limit.set(limit);
            if doc.borrow().selected.is_some() {
                update_counter(&counter_copy,&text(&editor_copy.buffer()),limit);
            }
        }
        gtk::glib::ControlFlow::Continue
    });

    main
}
