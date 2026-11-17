//! Bounded reader for the Named Binary Tag format used by Minecraft.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8), Short(i16), Int(i32), Long(i64), Float(f32), Double(f64),
    ByteArray(Vec<i8>), String(String),
    List { element_type: u8, items: Vec<Tag> },
    Compound(BTreeMap<String, Tag>), IntArray(Vec<i32>), LongArray(Vec<i64>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct NamedTag { pub name: String, pub value: Tag }

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_depth: usize,
    pub max_elements: usize,
    pub max_array_len: usize,
    pub max_string_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { max_depth: 64, max_elements: 1_000_000, max_array_len: 16 * 1024 * 1024, max_string_bytes: 65_535 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Eof, InvalidRoot(u8), InvalidTag(u8), InvalidLength(i32),
    Limit(&'static str), Utf8, Trailing(usize),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    limits: Limits,
    count: usize,
}
impl<'a> Reader<'a> {
    fn new(data: &'a [u8], limits: Limits) -> Self {
        Self { data, pos: 0, limits, count: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::Eof)?;
        let out = self.data.get(self.pos..end).ok_or(Error::Eof)?;
        self.pos = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, Error> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Result<i16, Error> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }
    fn i32(&mut self) -> Result<i32, Error> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i64(&mut self) -> Result<i64, Error> {
        let b = self.take(8)?;
        Ok(i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    fn string(&mut self) -> Result<String, Error> {
        let len = self.u16()? as usize;
        if len > self.limits.max_string_bytes { return Err(Error::Limit("string bytes")); }
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| Error::Utf8)
    }
    fn add_count(&mut self, n: usize) -> Result<(), Error> {
        self.count = self.count.checked_add(n).ok_or(Error::Limit("element count"))?;
        if self.count > self.limits.max_elements { return Err(Error::Limit("element count")); }
        Ok(())
    }
    fn length(&mut self) -> Result<usize, Error> {
        let n = self.i32()?;
        if n < 0 { return Err(Error::InvalidLength(n)); }
        let n = n as usize;
        if n > self.limits.max_array_len { return Err(Error::Limit("array/list length")); }
        Ok(n)
    }
    fn payload(&mut self, id: u8, depth: usize) -> Result<Tag, Error> {
        if depth > self.limits.max_depth { return Err(Error::Limit("nesting depth")); }
        self.add_count(1)?;
        match id {
            1 => Ok(Tag::Byte(self.u8()? as i8)),
            2 => Ok(Tag::Short(self.i16()?)),
            3 => Ok(Tag::Int(self.i32()?)),
            4 => Ok(Tag::Long(self.i64()?)),
            5 => Ok(Tag::Float(f32::from_bits(self.i32()? as u32))),
            6 => Ok(Tag::Double(f64::from_bits(self.i64()? as u64))),
            7 => {
                let n = self.length()?;
                self.add_count(n)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n { v.push(self.u8()? as i8); }
                Ok(Tag::ByteArray(v))
            }
            8 => Ok(Tag::String(self.string()?)),
            9 => {
                let ty = self.u8()?;
                let n = self.length()?;
                if ty > 12 || (ty == 0 && n != 0) { return Err(Error::InvalidTag(ty)); }
                let mut v = Vec::with_capacity(n);
                for _ in 0..n { v.push(self.payload(ty, depth + 1)?); }
                Ok(Tag::List { element_type: ty, items: v })
            }
            10 => {
                let mut v = BTreeMap::new();
                loop {
                    let ty = self.u8()?;
                    if ty == 0 { break; }
                    if ty > 12 { return Err(Error::InvalidTag(ty)); }
                    let name = self.string()?;
                    let child = self.payload(ty, depth + 1)?;
                    v.insert(name, child);
                }
                Ok(Tag::Compound(v))
            }
            11 => {
                let n = self.length()?;
                self.add_count(n)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n { v.push(self.i32()?); }
                Ok(Tag::IntArray(v))
            }
            12 => {
                let n = self.length()?;
                self.add_count(n)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n { v.push(self.i64()?); }
                Ok(Tag::LongArray(v))
            }
            other => Err(Error::InvalidTag(other)),
        }
    }
}

/// Parse one complete named root tag. Trailing bytes and oversized structures are rejected.
pub fn parse(data: &[u8]) -> Result<NamedTag, Error> {
    parse_with_limits(data, Limits::default())
}

pub fn parse_with_limits(data: &[u8], limits: Limits) -> Result<NamedTag, Error> {
    let mut r = Reader::new(data, limits);
    let id = r.u8()?;
    if !(1..=12).contains(&id) { return Err(Error::InvalidRoot(id)); }
    let name = r.string()?;
    let value = r.payload(id, 0)?;
    let trailing = r.data.len() - r.pos;
    if trailing != 0 { return Err(Error::Trailing(trailing)); }
    Ok(NamedTag { name, value })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_string(s: &str, out: &mut Vec<u8>) {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn parses_compound_and_named_integer() {
        let mut b = vec![10, 0, 0, 3];
        put_string("level", &mut b);
        b.extend_from_slice(&42_i32.to_be_bytes());
        b.push(0);
        let root = parse(&b).unwrap();
        assert_eq!(root.name, "");
        let Tag::Compound(children) = root.value else { panic!("not a compound") };
        assert_eq!(children.get("level"), Some(&Tag::Int(42)));
    }

    #[test]
    fn parses_list_and_signed_byte_values() {
        let mut b = vec![10, 0, 0, 9];
        put_string("values", &mut b);
        b.push(1);
        b.extend_from_slice(&2_i32.to_be_bytes());
        b.extend_from_slice(&[5, 255, 0]);
        let root = parse(&b).unwrap();
        let Tag::Compound(children) = root.value else { panic!("not a compound") };
        assert_eq!(children.get("values"), Some(&Tag::List {
            element_type: 1, items: vec![Tag::Byte(5), Tag::Byte(-1)]
        }));
    }

    #[test]
    fn rejects_negative_lengths_trailing_data_and_excess_depth() {
        let mut bad_array = vec![7, 0, 0];
        bad_array.extend_from_slice(&(-1_i32).to_be_bytes());
        assert_eq!(parse(&bad_array), Err(Error::InvalidLength(-1)));
        let trailing = vec![1, 0, 0, 7, 99];
        assert_eq!(parse(&trailing), Err(Error::Trailing(1)));
        let mut nested = vec![10, 0, 0, 10];
        put_string("nested", &mut nested);
        nested.push(0);
        let limits = Limits { max_depth: 0, ..Limits::default() };
        assert_eq!(parse_with_limits(&nested, limits), Err(Error::Limit("nesting depth")));
    }
}
