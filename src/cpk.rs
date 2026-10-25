use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct CpkEntry {
    pub path: String,
    pub offset: u64,
    pub packed_size: u64,
    pub unpacked_size: u64,
}

#[derive(Clone, Debug)]
enum Value { Number(u64), Text(String), Missing }

type Row = BTreeMap<String, Value>;

fn be16(b: &[u8], i: usize) -> Result<u16, String> {
    let p = b.get(i..i+2).ok_or("Truncated UTF integer")?;
    Ok(u16::from_be_bytes([p[0],p[1]]))
}
fn be32(b: &[u8], i: usize) -> Result<u32, String> {
    let p = b.get(i..i+4).ok_or("Truncated UTF integer")?;
    Ok(u32::from_be_bytes(p.try_into().unwrap()))
}
fn be64(b: &[u8], i: usize) -> Result<u64, String> {
    let p = b.get(i..i+8).ok_or("Truncated UTF integer")?;
    Ok(u64::from_be_bytes(p.try_into().unwrap()))
}
fn str_at(b: &[u8], i: usize) -> Result<String, String> {
    if i >= b.len() { return Err("UTF string offset out of range".into()); }
    let end = b[i..].iter().position(|&v|v==0).ok_or("Unterminated UTF string")? + i;
    std::str::from_utf8(&b[i..end]).map(|s|s.to_owned()).map_err(|e|e.to_string())
}
fn utf_value(b: &[u8], cursor: &mut usize, kind: u8, strings: usize) -> Result<Value,String> {
    let pos=*cursor;
    let (value,length)=match kind {
        0|1=>(Value::Number(*b.get(pos).ok_or("UTF short byte")? as u64),1),
        2|3=>(Value::Number(be16(b,pos)? as u64),2),
        4|5=>(Value::Number(be32(b,pos)? as u64),4),
        6|7=>(Value::Number(be64(b,pos)?),8),
        8=>(Value::Number(be32(b,pos)? as u64),4),
        9=>(Value::Number(be64(b,pos)?),8),
        10=>{
            let off=be32(b,pos)? as usize;
            (Value::Text(str_at(b,strings.checked_add(off).ok_or("UTF string overflow")?)?),4)
        },
        11=> {let _=b.get(pos..pos+8).ok_or("UTF data offset out of range")?; (Value::Missing,8)},
        _=>return Err(format!("Unsupported UTF type {kind}")),
    };
    *cursor=pos.checked_add(length).ok_or("UTF field overflow")?;
    Ok(value)
}
fn utf_rows(bytes: &[u8]) -> Result<Vec<Row>,String> {
    if !bytes.starts_with(b"@UTF") {return Err("CPK table lacks @UTF signature".into());}
    let len=(be32(bytes,4)? as usize).saturating_add(8).min(bytes.len());
    let b=&bytes[..len];
    let relative=8usize;
    let rows_pos=relative.checked_add(be32(b,8)? as usize).ok_or("UTF overflow")?;
    let strings=relative.checked_add(be32(b,12)? as usize).ok_or("UTF overflow")?;
    let columns=be16(b,24)? as usize;
    let row_len=be16(b,26)? as usize;
    let row_count=be32(b,28)? as usize;
    if columns>512 || row_count>2_000_000 || row_len>65535 {return Err("UTF dimensions exceed allowed range".into());}
    let mut pos=32usize;
    let mut schema=Vec::with_capacity(columns);
    for _ in 0..columns {
        let flags=*b.get(pos).ok_or("Truncated UTF schema")?;
        pos+=1;
        let name_offset=be32(b,pos)? as usize;
        pos+=4;
        let name=str_at(b, strings.checked_add(name_offset).ok_or("UTF overflow")?)?;
        let storage=flags&0xf0;
        let constant=if storage==0x30 {Some(utf_value(b,&mut pos,flags&0x0f,strings)?)} else {None};
        if !matches!(storage,0x00|0x10|0x30|0x50) {return Err(format!("Unknown UTF storage {storage:x}"));}
        schema.push((name,flags,constant));
    }
    let mut rows=Vec::with_capacity(row_count);
    for row in 0..row_count {
        let mut cursor=rows_pos.checked_add(row.checked_mul(row_len).ok_or("UTF rows overflow")?).ok_or("UTF rows overflow")?;
        if cursor> b.len() {return Err("UTF row out of range".into());}
        let mut values=Row::new();
        for (name,flags,constant) in &schema {
            let value=match flags&0xf0 {
                0x50=>utf_value(b,&mut cursor,flags&0x0f,strings)?,
                0x30=>constant.clone().unwrap_or(Value::Missing),
                _=>Value::Missing,
            };
            values.insert(name.clone(),value);
        }
        rows.push(values);
    }
    Ok(rows)
}

fn number(row:&Row,key:&str)->Option<u64> {match row.get(key){Some(Value::Number(n))=>Some(*n), _=>None}}
fn text<'a>(row:&'a Row,key:&str)->Option<&'a str> {match row.get(key){Some(Value::Text(s))=>Some(s),_=>None}}
fn read_packet(file: &mut File, start:u64) -> Result<Vec<u8>,String> {
    let len=file.metadata().map_err(|e|e.to_string())?.len();
    if start.saturating_add(24)>len {return Err("CPK packet outside source".into());}
    file.seek(SeekFrom::Start(start)).map_err(|e|e.to_string())?;
    let mut header=[0u8;16];
    file.read_exact(&mut header).map_err(|e|e.to_string())?;
    if &header[..4]!=b"CPK " && &header[..4]!=b"TOC " && &header[..4]!=b"ITOC" {
        return Err("Unsupported CPK section tag".into());
    }
    let packet_len=u64::from_le_bytes(header[8..16].try_into().unwrap());
    if packet_len>128*1024*1024 || start.saturating_add(16).saturating_add(packet_len)>len {
        return Err("CPK metadata packet is invalid or too large".into());
    }
    let mut bytes=vec![0;packet_len as usize];
    file.read_exact(&mut bytes).map_err(|e|e.to_string())?;
    if !bytes.starts_with(b"@UTF") {
        // CRI's XOR envelope; only the known UTF envelope is supported.
        let mut key=0x655fu32;
        for byte in &mut bytes {
            *byte ^= (key >> 8) as u8;
            key=key.wrapping_mul(0x4115);
        }
        if !bytes.starts_with(b"@UTF") { return Err("Unsupported or encrypted CPK table".into()); }
    }
    Ok(bytes)
}

pub fn index(path: &Path) -> Result<Vec<CpkEntry>,String> {
    let mut f=File::open(path).map_err(|e|format!("{}: {e}",path.display()))?;
    let header=utf_rows(&read_packet(&mut f,0)?)?;
    let row=header.first().ok_or("Empty CPK header")?;
    let toc=number(row,"TocOffset").ok_or("Missing TocOffset in CPK")?;
    let content=number(row,"ContentOffset").ok_or("Missing ContentOffset in CPK")?;
    if toc==u64::MAX || content==u64::MAX {return Err("Unsupported CPK without TOC".into());}
    let entries=utf_rows(&read_packet(&mut f,toc)?)?;
    let base=std::cmp::min(toc,content);
    let size=f.metadata().map_err(|e|e.to_string())?.len();
    let mut result=Vec::new();
    for row in entries {
        let file=text(&row,"FileName").ok_or("CPK entry without FileName")?;
        let dir=text(&row,"DirName").unwrap_or("");
        let rel=if dir.is_empty() {file.to_string()} else {format!("{dir}/{file}")};
        if rel.split('/').any(|s|s==".."||s.is_empty()) || rel.starts_with('/') || rel.contains('\\') || rel.contains(':') {
            return Err(format!("Unsafe CPK path: {rel}"));
        }
        let file_offset=number(&row,"FileOffset").ok_or("Missing FileOffset")?;
        let packed=number(&row,"FileSize").ok_or("Missing FileSize")?;
        let unpacked=number(&row,"ExtractSize").unwrap_or(packed);
        let absolute=base.checked_add(file_offset).ok_or("CPK offset overflow")?;
        if absolute.checked_add(packed).ok_or("CPK entry overflow")?>size {return Err(format!("CPK entry out of file bounds: {rel}"));}
        result.push(CpkEntry {path:rel,offset:absolute,packed_size:packed,unpacked_size:unpacked});
    }
    Ok(result)
}

struct BackwardBits<'a> {src:&'a [u8],pos:usize,pool:u8,left:u8}
impl<'a> BackwardBits<'a> {
    fn take(&mut self, mut n:u8)->Result<usize,String> {
        let mut v=0usize;
        while n!=0 {
            if self.left==0 {
                if self.pos<=16 {return Err("Truncated CRILAYLA bitstream".into());}
                self.pos-=1;
                self.pool=self.src[self.pos];
                self.left=8;
            }
            let step=n.min(self.left);
            self.left-=step;
            n-=step;
            v=(v<<step) | (((self.pool as usize >>self.left)&((1usize<<step)-1)));
        }
        Ok(v)
    }
}
fn crilayla(src: &[u8], expected: usize) -> Result<Vec<u8>,String> {
    if src.len()<0x110 || !src.starts_with(b"CRILAYLA") {return Err("Invalid CRILAYLA frame".into());}
    let body=u32::from_le_bytes(src[8..12].try_into().unwrap()) as usize;
    let off=u32::from_le_bytes(src[12..16].try_into().unwrap()) as usize;
    if body.checked_add(0x100)!=Some(expected) || off.checked_add(0x110)!=Some(src.len()) {return Err("CRILAYLA size mismatch".into());}
    let mut out=vec![0u8;expected];
    out[..256].copy_from_slice(&src[src.len()-256..]);
    let mut bits=BackwardBits{src,pos:16+off,pool:0,left:0};
    let mut pos=expected;
    while pos>256 {
        if bits.take(1)?==0 {
            pos-=1;
            out[pos]=bits.take(8)? as u8;
        } else {
            let distance=bits.take(13)?+3;
            let mut length=3usize;
            let mut exhausted=true;
            for width in [2,3,5,8] {
                let value=bits.take(width)?;
                length+=value;
                if value!=(1<<width)-1 {exhausted=false; break;}
            }
            if exhausted {
                loop {let value=bits.take(8)?;length+=value;if value!=255 {break;}}
            }
            if pos<256+length || pos+distance>expected {return Err("CRILAYLA invalid reference".into());}
            for _ in 0..length {pos-=1;out[pos]=out[pos+distance];}
        }
    }
    Ok(out)
}

pub fn read_entry(file:&mut File,entry:&CpkEntry) -> Result<Vec<u8>,String> {
    if entry.unpacked_size>256*1024*1024 || entry.packed_size>256*1024*1024 {return Err("CPK entry too large".into());}
    file.seek(SeekFrom::Start(entry.offset)).map_err(|e|e.to_string())?;
    let mut data=vec![0u8;entry.packed_size as usize];
    file.read_exact(&mut data).map_err(|e|e.to_string())?;
    if data.starts_with(b"CRILAYLA") {crilayla(&data,entry.unpacked_size as usize)}
    else if entry.packed_size==entry.unpacked_size {Ok(data)}
    else {Err(format!("Unsupported compression for {}",entry.path))}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn utf_header_guards(){ assert!(utf_rows(b"@UTF").is_err()); }
    #[test] fn corrupted_layla_guards(){ assert!(crilayla(b"CRILAYLA",256).is_err()); }
    #[test] fn short_backward_bits(){ let mut b=BackwardBits{src:b"abcdefghij01234567890",pos:17,pool:0,left:0};assert!(b.take(16).is_err());}
}
