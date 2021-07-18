use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::unix::prelude::OsStringExt;
use std::path::PathBuf;

#[derive(Debug)]
pub struct HeaderNamePathPair {
    name: HeaderName,
    path: PathBuf,
}

impl HeaderNamePathPair {
    pub fn try_from_os_string(s: &OsStr) -> Result<HeaderNamePathPair, OsString> {
        let s_as_vec = s.to_os_string().into_vec();
        let mut components = s_as_vec.splitn(2, |c| *c == 0x3a); // 0x3a == ':'
        let (h, v) = components
            .next()
            .and_then(|h| components.next().map(|v| (h, v)))
            .ok_or(
                "no : found, need form header_name:path_to_file_containing_header_value_per_line",
            )?;
        let name = HeaderName::from_bytes(h).map_err(|v| format!("header name invalid: {}", v))?;
        let path = PathBuf::from(OsString::from_vec(v.to_vec()));
        Ok(HeaderNamePathPair { name, path })
    }
}

#[derive(Debug)]
pub struct HeaderValues {
    name: HeaderName,
    values: Vec<HeaderValue>,
}

pub struct HeaderValuesCycle<'a> {
    cycles: Vec<(
        HeaderName,
        std::iter::Cycle<std::slice::Iter<'a, HeaderValue>>,
    )>,
}

impl Iterator for HeaderValuesCycle<'_> {
    type Item = HeaderMap;
    fn next(&mut self) -> Option<HeaderMap> {
        let mut result = HeaderMap::new();
        for &mut (ref mut name, ref mut cycle) in self.cycles.iter_mut() {
            match cycle.next() {
                None => return None,
                Some(val) => {
                    result.append(name.clone(), val.clone());
                }
            }
        }
        Some(result)
    }
}

impl HeaderValues {
    pub fn new(HeaderNamePathPair { name, path }: HeaderNamePathPair) -> Result<HeaderValues, String> {
        let mut values: Vec<HeaderValue> = Vec::new();
        let file = File::open(path).map_err(|v| v.to_string())?;
        let mut buf_reader = BufReader::new(file);
        let mut byte_line: Vec<u8> = Vec::new();
        while 0
            != buf_reader
                .read_until(0x0a, &mut byte_line)
                .map_err(|v| v.to_string())?
        {
            if let Some((last, rest)) = byte_line.split_last() {
                let header_value_bytes = if *last == 0x0a { rest } else { &byte_line[..] };
                values
                    .push(HeaderValue::from_bytes(header_value_bytes).map_err(|v| {
                        format!("{}: '{}'", v, String::from_utf8_lossy(&byte_line))
                    })?);
                byte_line.clear();
            }
        }
        Ok(HeaderValues { name, values })
    }
}

pub fn cycle_headers(header_values: &[HeaderValues]) -> HeaderValuesCycle {
    HeaderValuesCycle {
        cycles: header_values
            .iter()
            .map(|HeaderValues { name, values }| (name.clone(), values.iter().cycle()))
            .collect(),
    }
}

