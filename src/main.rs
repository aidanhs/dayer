extern crate tar;

use std::collections::HashSet;
use std::env;
use std::fs::File;
use std::hash::{Hash, Hasher};
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
    fn as_bytes(&self) -> &[u8; 512] {
        unsafe { &*(&self.0 as *const _ as *const [u8; 512]) }
    }
}
impl Hash for HashableHeader {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}
impl PartialEq for HashableHeader {
    fn eq(&self, other: &HashableHeader) -> bool {
        self.as_bytes().iter().zip(other.as_bytes().iter()).all(|(a, b)| a == b)
    }
}
impl Eq for HashableHeader {}

fn get_initial_filelist(a: &Archive<File>) -> HashSet<HashableHeader> {
    let mut afiles: HashSet<HashableHeader> = HashSet::new();
    for file in a.files().unwrap() {
        // Make sure there wasn't an I/O error
        let file = file.unwrap();
        // Get file metadata
        afiles.insert(HashableHeader::new(file.header()));
    }
    afiles
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
    let file1 = File::open(tname1).unwrap();
    let ar1 = Archive::new(file1);
    let afiles1 = get_initial_filelist(&ar1);
    println!("Loading {}: found {} files", tname1, afiles1.len());

    println!("Loading {}", tname2);
    let file2 = File::open(tname2).unwrap();
    let ar2 = Archive::new(file2);
    let afiles2 = get_initial_filelist(&ar2);
    println!("Loading {}: found {} files", tname2, afiles2.len());

    println!("Phase 1: metadata compare");
    let p1result: Vec<_> = afiles1.intersection(&afiles2).collect();
    let p1size = p1result.iter().fold(0, |sum, h| sum + h.0.size().unwrap());
    let p1sizestr = format_num_bytes(p1size);
    println!("Phase 1 complete: {} files with {}", p1result.len(), p1sizestr);

    // prune dirs
}
