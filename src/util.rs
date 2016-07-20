use std::collections::HashMap;
use std::hash::Hash;
use std::io;
use std::io::prelude::*;

pub fn find_common_keys<K, V>(hms: &[HashMap<K, V>]) -> Vec<K>
    where K: Clone + Eq + Hash
{
    let mut keycount: HashMap<&K, usize> = HashMap::new();
    for key in hms.iter().flat_map(|hm| hm.keys()) {
        let counter = keycount.entry(key).or_insert(0);
        *counter += 1;
    }
    let numhms = hms.len();
    keycount.iter()
            .filter_map(|(key, count)| {
                if *count != numhms {
                    None
                } else {
                    Some((*key).clone())
                }
            })
            .collect()
}

pub fn format_num_bytes(num: u64) -> String {
    if num > 99 * 1024 * 1024 {
        format!("~{}MB", num / 1024 / 1024)
    } else if num > 99 * 1024 {
        format!("~{}KB", num / 1024)
    } else {
        format!("~{}B", num)
    }
}

pub fn readers_identical<R>(rs: &mut [R]) -> bool
    where R: Read
{
    // This approach is slow:
    // if f1.bytes().zip(f2.bytes()).all(|(b1, b2)| b1.unwrap() == b2.unwrap()) {
    let mut brs: Vec<_> = rs.iter_mut()
                            .map(|r| io::BufReader::with_capacity(512, r))
                            .collect();
    loop {
        let numread = {
            let bufs: Vec<_> = brs.iter_mut()
                                  .map(|buf| buf.fill_buf().unwrap())
                                  .collect();
            let basebuf = bufs[0];
            let numread = basebuf.len();
            if numread == 0 {
                return true;
            }
            if !bufs.iter().all(|buf| &basebuf == buf) {
                return false;
            }
            numread
        };
        for br in &mut brs {
            br.consume(numread);
        }
    }
}

pub fn to_string_slices(strings: &[String]) -> Vec<&str> {
    strings.iter().map(|s| &s[..]).collect()
}
