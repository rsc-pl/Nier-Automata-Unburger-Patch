use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct TranslationRow {
    pub key: String,
    pub value: String,
    pub original_value: String,
    pub source_line: usize,
    pub dirty: bool,
}

#[derive(Clone, Debug)]
pub struct TranslationDocument {
    source: String,
    plain: bool,
    pub rows: Vec<TranslationRow>,
}

fn decode(input: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' { out.push(ch); continue; }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('u') => {
                let hex = chars.by_ref().take(4).collect::<String>();
                let code = u16::from_str_radix(&hex, 16)
                    .map_err(|_| format!("Invalid unicode escape: \\u{hex}"))?;
                out.push(char::from_u32(code as u32).ok_or("Invalid Unicode scalar")?);
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    Ok(out)
}

fn encode(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out
}

impl TranslationDocument {
    pub fn open(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(text)
    }

    pub fn parse(source: String) -> Result<Self, String> {
        let mut rows = Vec::new();
        let mut seen = BTreeSet::new();
        for (line_number, line) in source.lines().enumerate() {
            // Preserve comment and blank lines without treating them as editable records.
            if line.trim().is_empty() || line.trim_start().starts_with('#') || line.trim_start().starts_with('!') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue; };
            let key = decode(key).map_err(|e| format!("Line {}: {e}", line_number + 1))?;
            let value = decode(value).map_err(|e| format!("Line {}: {e}", line_number + 1))?;
            if !seen.insert(key.clone()) {
                return Err(format!("Duplicate key at line {}: {key}", line_number + 1));
            }
            rows.push(TranslationRow { key, original_value: value.clone(), value, source_line: line_number, dirty: false });
        }
        Ok(Self { source, plain: false, rows })
    }

    pub fn plain(source: String) -> Self {
        Self { rows: vec![TranslationRow { key: "Document".into(), original_value: source.clone(), value: source.clone(), source_line: 0, dirty: false }], source, plain: true }
    }

    pub fn update(&mut self, index: usize, value: String) -> Result<(), String> {
        let row = self.rows.get_mut(index).ok_or("Invalid translation row")?;
        row.dirty = value != row.original_value;
        row.value = value;
        Ok(())
    }

    pub fn dirty(&self) -> bool { self.rows.iter().any(|r| r.dirty) }

    pub fn render(&self) -> String {
        if self.plain { return self.rows[0].value.clone(); }
        let mut by_line = std::collections::BTreeMap::new();
        for row in &self.rows {
            if row.dirty { by_line.insert(row.source_line, (&row.key, &row.value)); }
        }
        if by_line.is_empty() { return self.source.clone(); }
        let mut out = String::new();
        for (line_number, segment) in self.source.split_inclusive('\n').enumerate() {
            if let Some((_, value)) = by_line.get(&line_number) {
                let content = segment.trim_end_matches('\n');
                let line_ending = if content.ends_with('\r') { "\r\n" } else if segment.ends_with('\n') { "\n" } else { "" };
                let raw = content.trim_end_matches('\r');
                let prefix = raw.split_once('=').map(|(k, _)| k).unwrap_or("");
                out.push_str(prefix);
                out.push('=');
                out.push_str(&encode(value));
                out.push_str(line_ending);
            } else {
                out.push_str(segment);
            }
        }
        out
    }

    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        if !self.dirty() { return Ok(()); }
        let new_text = self.render();
        // Validate against exactly the parser the builder uses before touching the file.
        let checked = if self.plain { Self::plain(new_text.clone()) } else { Self::parse(new_text.clone())? };
        if checked.rows.len() != self.rows.len() {
            return Err("Saving would change the number of entries".into());
        }
        for (before, after) in self.rows.iter().zip(checked.rows.iter()) {
            if before.key != after.key || before.value != after.value {
                return Err(format!("Round-trip mismatch for {}", before.key));
            }
        }
        let temp = path.with_extension("properties.unburger-tmp");
        fs::write(&temp, &new_text).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        if path.exists() { fs::remove_file(path).map_err(|e| e.to_string())?; }
        fs::rename(&temp, path).map_err(|e| e.to_string())?;
        *self = checked;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noop_is_byte_identical() {
        let src = "# A\r\nA=hello\\nworld\r\nB=x\\\\y\r\n";
        let doc = TranslationDocument::parse(src.into()).unwrap();
        assert_eq!(doc.render(), src);
        assert_eq!(doc.rows[0].value, "hello\nworld");
    }
    #[test]
    fn edit_preserves_other_lines_and_escapes() {
        let mut doc = TranslationDocument::parse("# a\nA=Old\nB=keep\\tme\n".into()).unwrap();
        doc.update(0,"New\nvalue".into()).unwrap();
        assert_eq!(doc.render(),"# a\nA=New\\nvalue\nB=keep\\tme\n");
    }
    #[test]
    fn undoing_an_edit_restores_original_bytes() {
        let input="key=Original\\nvalue\r\n";
        let mut doc=TranslationDocument::parse(input.to_string()).unwrap();
        doc.update(0,"Changed".into()).unwrap();
        assert!(doc.dirty());
        doc.update(0,"Original\nvalue".into()).unwrap();
        assert!(!doc.dirty());
        assert_eq!(doc.render(),input);
    }
    #[test]
    fn duplicate_keys_are_rejected() {
        assert!(TranslationDocument::parse("A=x\nA=y\n".into()).is_err());
    }
}
