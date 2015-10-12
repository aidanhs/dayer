extern crate tar;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::prelude::*;
use std::ptr;
use tar::{Header, Archive};

// https://github.com/rust-lang/rust/issues/13721
struct HashableHeader(Header);
impl HashableHeader {
    pub fn new(srcheader: &Header) -> HashableHeader {
        let mut header: Header = Header::new();
        unsafe { ptr::copy_nonoverlapping(srcheader, &mut header as *mut Header, 1) };
        return HashableHeader(header);
    }
    // stolen from tar-rs
    fn head_bytes(&self) -> &[u8; 512] {
        unsafe { &*(&self.0 as *const _ as *const [u8; 512]) }
    }
}
impl Hash for HashableHeader {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.head_bytes().hash(state);
    }
}
impl PartialEq for HashableHeader {
    fn eq(&self, other: &HashableHeader) -> bool {
        self.head_bytes()[..] == other.head_bytes()[..]
    }
}
impl Clone for HashableHeader {
    fn clone(&self) -> HashableHeader {
        HashableHeader::new(&self.0)
    }
}
impl Eq for HashableHeader {}

fn get_header_map<'a>(arfiles: &'a mut Vec<tar::File<'a, fs::File>>) -> HashMap<HashableHeader, &'a mut tar::File<'a, fs::File>> {
    let mut arfilemap: HashMap<HashableHeader, &'a mut tar::File<'a, fs::File>> = HashMap::new();
    for file in arfiles.iter_mut() {
        arfilemap.insert(HashableHeader::new(file.header()), file);
    }
    arfilemap
}

fn format_num_bytes(num: u64) -> String {
    if num > 99 * 1024 * 1024 {
        format!("~{}MB", num / 1024 / 1024)
    } else if num > 99 * 1024 {
        format!("~{}KB", num / 1024)
    } else {
        format!("~{}B", num)
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        println!("Invalid number of args: {}", args.len());
    }
    let tname1 = &args[1];
    let tname2 = &args[2];

    println!("Loading {}", tname1);
    let file1 = fs::File::open(tname1).unwrap();
    let ar1 = Archive::new(file1);
    let mut arfiles1: Vec<_> = ar1.files().unwrap().map(|res| res.unwrap()).collect();
    println!("Loading {}: found {} files", tname1, arfiles1.len());

    println!("Loading {}", tname2);
    let file2 = fs::File::open(tname2).unwrap();
    let ar2 = Archive::new(file2);
    let mut arfiles2: Vec<_> = ar2.files().unwrap().map(|res| res.unwrap()).collect();
    println!("Loading {}: found {} files", tname2, arfiles2.len());

    println!("Phase 1: metadata compare");
    let mut arheadmap1 = get_header_map(&mut arfiles1);
    let mut arheadmap2 = get_header_map(&mut arfiles2);
    // ideally would be &HashableHeader, but that borrows the maps as immutable
    // which then conflicts with the mutable borrow later because a borrow of
    // either keys or values applies to the whole hashmap
    // https://github.com/rust-lang/rfcs/issues/1215
    let p1result: Vec<HashableHeader> = arheadmap1
        .keys().filter(|k| arheadmap2.contains_key(k)).map(|k| k.clone()).collect();
    let p1size = p1result.iter().fold(0, |sum, h| sum + h.0.size().unwrap());
    let p1sizestr = format_num_bytes(p1size);
    println!("Phase 1 complete: {} files with {}", p1result.len(), p1sizestr);

    println!("Phase 2: data compare");
    let mut p2result: Vec<HashableHeader> = vec![];
    for (i, hheader) in p1result.iter().enumerate() {
        let f1: &mut tar::File<fs::File> = arheadmap1.get_mut(hheader).unwrap();
        let f2: &mut tar::File<fs::File> = arheadmap2.get_mut(hheader).unwrap();
        // Do the files have the same contents?
        // Note we've verified they have the same size by now
        let mut bf1 = io::BufReader::new(f1);
        let mut bf2 = io::BufReader::new(f2);
        loop {
            let minsize = {
                let buf1 = bf1.fill_buf().unwrap();
                let buf2 = bf2.fill_buf().unwrap();
                let minsize = if buf1.len() < buf2.len() { buf1.len() } else { buf2.len() };
                if minsize == 0 {
                    assert!(buf1.len() == 0 && buf2.len() == 0);
                    p2result.push(hheader.clone());
                    break
                }
                if buf1[0..minsize] != buf2[0..minsize] {
                    break
                }
                minsize
            };
            bf1.consume(minsize);
            bf2.consume(minsize);
        }
        print!("\r    Done {}", i);
        io::stdout().flush().unwrap();
    }
    println!("");
    let p2size = p2result.iter().fold(0, |sum, h| sum + h.0.size().unwrap());
    let p2sizestr = format_num_bytes(p2size);
    println!("Phase 2 complete: {} files with {}", p2result.len(), p2sizestr);

    // prune dirs
}
