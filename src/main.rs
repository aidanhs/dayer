#![feature(path_relative_from)]

// For test decorators
#![feature(plugin, custom_attribute)]
#![plugin(adorn)]
// Only allow one test directory at a time
#![feature(static_mutex)]
// Literal maps for test purposes
#[cfg(test)]
#[macro_use] extern crate maplit;

extern crate tar;

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use std::process;
use std::str;
use tar::{Header, Archive};

// https://github.com/rust-lang/rust/issues/13721
#[derive(Clone)]
struct HashableHeader(Header);
impl HashableHeader {
    pub fn new(srcheader: &Header) -> HashableHeader {
        return HashableHeader(srcheader.clone());
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
impl Eq for HashableHeader {}

// octal_from in tar-rs
fn truncate<'a>(slice: &'a [u8]) -> &'a [u8] {
    match slice.iter().position(|i| *i == 0) {
        Some(i) => &slice[..i],
        None => slice,
    }
}
fn decimal_from(slice: &[u8]) -> io::Result<u64> {
    let num = match str::from_utf8(truncate(slice)) {
        Ok(n) => n,
        Err(_) => panic!("noo"),
    };
    match u64::from_str_radix(num.trim(), 10) {
        Ok(n) => Ok(n),
        Err(_) => panic!("noo"),
    }
}
fn parse_extended_header_data(extended_header: &[u8]) -> HashMap<&str, &str> {
    let mut data = extended_header;
    let mut outmap: HashMap<&str, &str> = HashMap::new();
    while data.len() != 0 {
        let spacepos: usize = data.iter().position(|c| *c == b' ').unwrap();
        let (sizeslice, restdata) = data.split_at(spacepos);
        let size = decimal_from(sizeslice).unwrap();
        let (spacekvslice, restdata2) = restdata.split_at(size as usize-sizeslice.len());
        let kvslice = &spacekvslice[1..spacekvslice.len()-1];
        let eqpos: usize = kvslice.iter().position(|c| *c == b'=').unwrap();
        let (key, eqval) = kvslice.split_at(eqpos);
        let val = &eqval[1..];
        assert!(outmap.insert(str::from_utf8(key).unwrap(), str::from_utf8(val).unwrap()).is_none());
        data = restdata2
    }
    outmap
}

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

fn make_layer_tar<'a, 'b: 'a, I1: Iterator<Item=&'a HashableHeader>, I2: Iterator<Item=&'a mut tar::File<'b, fs::File>>, F: Fn(&Path) -> tar::Header>(
        outname: &str,
        headeriter: I1,
        verbatimiter: I2,
        headertofilemap: &mut HashMap<HashableHeader, &mut tar::File<fs::File>>,
        mkdir: F) {

    let outfile = fs::File::create(outname).unwrap();
    let outar = Archive::new(outfile);

    // Alphabetical ordering, lets us make assumptions about directory traversal
    let mut headers: Vec<&HashableHeader> = headeriter.collect();
    headers.sort_by(|h1, h2| h1.0.path_bytes().cmp(&h2.0.path_bytes()));

    let mut lastdir = PathBuf::new();
    // TODO: set trailing slash of dirs for belt and braces?
    for hheader in headers.iter() {
        let header = &hheader.0;
        assert!(&header.ustar[..5] == b"ustar"); // TODO: get this as public?
        let path = header.path().unwrap();
        // Climb up to find common prefix
        while !path.starts_with(&lastdir) {
            lastdir = lastdir.parent().unwrap().to_path_buf();
        }
        // Climb down creating dirs as necessary
        let relpath = path.parent().unwrap().relative_from(&lastdir).unwrap().to_path_buf();
        for relcomponent in relpath.iter() {
            lastdir.push(relcomponent);
            let newdir = mkdir(&lastdir);
            outar.append(&newdir, &mut io::empty()).unwrap();
        }
        let file = headertofilemap.get_mut(&hheader).unwrap();
        outar.append(&header, file).unwrap();
        file.seek(io::SeekFrom::Start(0)).unwrap();
        if header.link[0] == b'5' {
            lastdir = path.to_path_buf();
        }
    }
    for af in verbatimiter {
        let hheader = HashableHeader::new(af.header()).clone();
        outar.append(&hheader.0, af).unwrap();
        af.seek(io::SeekFrom::Start(0)).unwrap();
    }

    outar.finish().unwrap();
}

// TODO
// - check ustar at beginning
// - check paths are not absolute
// - be more intelligent about dirs - no point storing one child dir in common
//   tar because we have to store the parents as well, and then have to
//   overwrite the parents in specific tar
// - implement rebasing 'onto' an image, with deletes for irrelevant files etc
// - how do directory overwrites work in docker layers? e.g. if you chmod it,
//   presumably it will pull parent directories up from the previous layer, does
//   it grab children files as well?
// - assert not more than one of the same name
// - report files missed because of extended headers
// - assert sane sequence of headers (x is followed by a normal file)
// - handle extended headers
// - assert it's a posix archives (i.e. dirs use type 5 rather than 1)
// - ensure hard links don't get split across archives

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("No operation specified");
        process::exit(1);
    }
    if args[1] == "commonise" {
        println!("Invalid operation - use shell script to commonise");
        process::exit(1);
    }
    if args[1] != "commonise-tar" {
        println!("Invalid operation - only commonise-tar supported");
        process::exit(1);
    }
    if args.len() < 4 {
        println!("Invalid number of args: {}", args.len());
        process::exit(1);
    }
    let slargs: Vec<&str> = args[2..].iter().map(|s| &s[..]).collect();
    commonise_tars(&slargs[..]);
}

pub fn commonise_tars(tnames: &[&str]) {
    let numars = tnames.len();

    println!("Opening tars");
    let ars: Vec<tar::Archive<fs::File>> = tnames.iter().map(|tname| {
        let file = fs::File::open(tname).unwrap();
        Archive::new(file)
    }).collect();
    let mut ignoredfiless: Vec<Vec<tar::File<fs::File>>> = vec![];
    let mut arfiless: Vec<Vec<tar::File<fs::File>>> = ars.iter().zip(tnames).map(|(ar, tname)| {
        println!("Loading {}", tname);
        let mut ignoredfiles: Vec<tar::File<fs::File>> = vec![];
        // Can't handle extended headers at the moment - skip the next block if
        // prefixed by an extended header
        let mut skipnext = false;
        let mut extpath = PathBuf::new();
        let emptypath = PathBuf::new();
        // If we've skipped directories because of an extended header, exclude
        // anything under that
        let mut skipdirs: HashSet<PathBuf> = HashSet::new();
        let arfiles: Vec<_> = ar.files().unwrap().filter_map(|res| {
            let af = res.unwrap();
            let ftype = af.header().link[0];
            // Handle extended headers, skip other headers if necessary
            if ftype == b'g' {
                panic!("Cannot handle global extended header")
            } else if ftype == b'x' {
                assert!(!skipnext && extpath == emptypath);
                skipnext = true;
                let mut extdata = vec![];
                unsafe { // TODO: just to dodge mutability requirement
                    let afm = &mut *(&af as *const tar::File<fs::File> as *mut tar::File<fs::File>);
                    afm.read_to_end(&mut extdata).unwrap();
                    afm.seek(io::SeekFrom::Start(0)).unwrap();
                }
                let extheadmap = parse_extended_header_data(&extdata);
                if extheadmap.contains_key("path") {
                    extpath = PathBuf::from(extheadmap["path"]);
                }
                ignoredfiles.push(af);
                None
            // http://stackoverflow.com/questions/2078778/what-exactly-is-the-gnu-tar-longlink-trick
            // https://golang.org/pkg/archive/tar/
            } else if b'A' <= ftype && ftype <= b'Z' {
                panic!("Unknown vendor-specific header: {}", ftype as char)
            } else if skipnext {
                if ftype == b'5' { // dir
                    let headpath = af.header().path().unwrap().to_path_buf();
                    assert!(
                        ((headpath == emptypath) ^ (extpath == emptypath)) ||
                        extpath.to_str().unwrap().starts_with(headpath.to_str().unwrap())
                    );
                    let path = if extpath != emptypath { &extpath } else { &headpath };
                    // Normalise it https://github.com/rust-lang/rust/issues/29008
                    skipdirs.insert(path.components().as_path().to_path_buf());
                }
                skipnext = false;
                extpath = emptypath.clone();
                ignoredfiles.push(af);
                None
            } else {
                // Does the path need to be skipped because a parent is skipped?
                {
                    let path = af.header().path().unwrap().to_path_buf();
                    assert!(path != emptypath);
                    let mut prefix = path.parent();
                    while prefix != None {
                        let p = prefix.unwrap();
                        if skipdirs.contains(p) {
                            ignoredfiles.push(af);
                            return None
                        }
                        prefix = p.parent();
                    }
                }
                Some(af)
            }
        }).collect();
        println!("Loading {}: found {} files, ignored {}",
                 tname, arfiles.len(), ignoredfiles.len());
        ignoredfiless.push(ignoredfiles);
        arfiles
    }).collect();

    println!("Phase 1: metadata compare");
    let mut arheadmaps: Vec<HashMap<HashableHeader, &mut tar::File<fs::File>>> =
        arfiless.iter_mut().map(|arfiles| get_header_map(arfiles)).collect();
    // ideally would be &HashableHeader, but that borrows the maps as immutable
    // which then conflicts with the mutable borrow later because a borrow of
    // either keys or values applies to the whole hashmap
    // https://github.com/rust-lang/rfcs/issues/1215
    let p1result: Vec<HashableHeader> = {
        let mut headercount: HashMap<&HashableHeader, usize> = HashMap::new();
        for key in arheadmaps.iter().flat_map(|hm| hm.keys()) {
          let counter = headercount.entry(key).or_insert(0);
          *counter += 1;
        }
        headercount.iter().filter_map(|(hheader, count)| {
            if *count != numars { None } else { Some((*hheader).clone()) }
        }).collect()
    };
    let p1size = p1result.iter().fold(0, |sum, h| sum + h.0.size().unwrap());
    let p1sizestr = format_num_bytes(p1size);
    println!("Phase 1 complete: possible {} files with {}", p1result.len(), p1sizestr);

    println!("Phase 2: data compare");
    let mut p2result: Vec<HashableHeader> = vec![];
    // TODO: sort by offset in archive? means not seeking backwards
    for (i, hheader) in p1result.iter().enumerate() {
        let mut files: Vec<&mut &mut tar::File<fs::File>> = arheadmaps.iter_mut().map(|arh|
            arh.get_mut(hheader).unwrap()
        ).collect();
        // Do the files have the same contents?
        // Note we've verified they have the same size by now
        // This approach is slow:
        //     if f1.bytes().zip(f2.bytes()).all(|(b1, b2)| b1.unwrap() == b2.unwrap()) {
        let mut buffiles: Vec<_> = files.iter_mut().map(|f|
            io::BufReader::with_capacity(512, f)
        ).collect();
        loop {
            let numread = {
                let bufs: Vec<&[u8]> = buffiles.iter_mut().map(|bf| bf.fill_buf().unwrap()).collect();
                let basebuf = bufs[0];
                let numread = basebuf.len();
                if numread == 0 {
                    p2result.push(hheader.clone());
                    break
                }
                if !bufs.iter().all(|buf| &basebuf == buf) {
                    break
                }
                numread
            };
            for bf in buffiles.iter_mut() {
                bf.consume(numread);
            }
        }
        if i % 100 == 0 {
            print!("    Done {}\r", i);
        }
        io::stdout().flush().unwrap();
        // Leave the file how we found it
        for bf in buffiles.iter_mut() {
            bf.seek(io::SeekFrom::Start(0)).unwrap();
        }
    }
    let p2size = p2result.iter().fold(0, |sum, h| sum + h.0.size().unwrap());
    let p2sizestr = format_num_bytes(p2size);
    println!("Phase 2 complete: actual {} files with {}", p2result.len(), p2sizestr);

    println!("Phase 3a: preparing for layer creation");
    let tonormpath = |h: &HashableHeader| {
        // Normalise it https://github.com/rust-lang/rust/issues/29008
        h.0.path().unwrap().components().as_path().to_path_buf()
    };
    let commonmap: HashMap<PathBuf, &HashableHeader> = p2result
        .iter().map(|h| (tonormpath(h), h)).collect();
    println!("Phase 3a complete");

    println!("Phase 3b: common layer creation");
    // Create a holding-place directory for the common layer as it will be
    // overwritten by the layer above
    let minimalmkdir = |dirpath: &Path| {
        let mut newdir = tar::Header::new();
        newdir.set_path(&dirpath).unwrap();
        // https://github.com/docker/docker/issues/783
        newdir.set_mode(0o777);
        newdir.set_uid(0);
        newdir.set_gid(0);
        newdir.set_mtime(0);
        // cksum: calculated below
        newdir.link[0] = b'5'; // dir
        // linkname: irrelevant
        newdir.set_cksum();
        newdir
    };
    let outname = "common.tar";
    // It doesn't matter which head map, these are common files!
    make_layer_tar(outname, p2result.iter(), vec![].iter_mut(), arheadmaps.get_mut(0).unwrap(), &minimalmkdir);
    println!("Phase 3b complete: created {}", outname);

    println!("Phase 3c: individual layer creation");
    let thievingmkdir = |dirpath: &Path| {
        commonmap[dirpath].clone().0
    };
    for (i, (arheadmap, ignoredfiles)) in arheadmaps.iter_mut().zip(ignoredfiless.iter_mut()).enumerate() {
      let outname = format!("individual_{}.tar", i);
      let outheads: Vec<_> = arheadmap.keys()
          .filter(|h| !commonmap.contains_key(&tonormpath(h)))
          .map(|h| h.clone())
          .collect();
      make_layer_tar(&outname, outheads.iter(), ignoredfiles.iter_mut(), arheadmap, &thievingmkdir);
    }
    println!("Phase 3c complete: created {} individual tars", arheadmaps.len());
}

#[cfg(test)]
mod tests {
    extern crate tempdir;

    use std::collections::HashMap;
    use std::env::set_current_dir;
    use std::fs;
    use std::io::prelude::*;
    use std::sync::{StaticMutex, MUTEX_INIT};

    use self::tempdir::TempDir;
    use self::DirTreeEntry::*;
    use super::tar::Archive;

    use super::*;

    macro_rules! t {
        ($e:expr) => (match $e {
            Ok(v) => v,
            Err(e) => panic!("{} returned {}", stringify!($e), e),
        })
    }

    static TMPLOCK: StaticMutex = MUTEX_INIT;

    // Does not put program back in original dir
    fn intmp<F>(f: F) where F: Fn() {
        let mut _guard = match TMPLOCK.lock() { // destroyed at end of fn
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let td = TempDir::new("dayer").unwrap(); // destroyed at end of fn
        set_current_dir(td.path()).unwrap();
        f();
    }

    enum DirTreeEntry<'a> {
        F(&'a str),
        D,
    }

    #[test]
    #[adorn(intmp)]
    fn empty_tars() {
        let filetree = hashmap!{};
        let infilelists = hashmap!{
            "in0.tar" => vec![],
            "in1.tar" => vec![],
        };
        let outfilelists = hashmap!{
            "common.tar" => vec![],
            "individual_0.tar" => vec![],
            "individual_1.tar" => vec![],
        };
        test_commonise(filetree, infilelists, outfilelists);
    }

    #[test]
    #[adorn(intmp)]
    fn simple_tars() {
        let filetree = hashmap!{
            "0" => F("0content"),
            "1" => F("1content"),
            "common" => F("commoncontent"),
        };
        let infilelists = hashmap!{
            "in1.tar" => vec!["common", "0"],
            "in2.tar" => vec!["common", "1"],
        };
        let outfilelists = hashmap!{
            "common.tar" => vec!["common"],
            "individual_0.tar" => vec!["0"],
            "individual_1.tar" => vec!["1"],
        };
        test_commonise(filetree, infilelists, outfilelists);
    }

    #[test]
    #[adorn(intmp)]
    fn leading_dirs() {
        let filetree = hashmap!{
            "dir" => D,
            "dir/0" => F("0content"),
            "dir/1" => F("1content"),
            "common" => F("commoncontent"),
        };
        let infilelists = hashmap!{
            "in1.tar" => vec!["common", "dir", "dir/0"],
            "in2.tar" => vec!["common", "dir", "dir/1"],
        };
        let outfilelists = hashmap!{
            "common.tar" => vec!["common", "dir"],
            "individual_0.tar" => vec!["dir", "dir/0"],
            "individual_1.tar" => vec!["dir", "dir/1"],
        };
        test_commonise(filetree, infilelists, outfilelists);
    }

    fn test_commonise(filetree: HashMap<&str, DirTreeEntry>,
                      infilelists: HashMap<&str, Vec<&str>>,
                      outfilelists: HashMap<&str, Vec<&str>>) {

        let mut fpaths: Vec<&str> = filetree.keys().map(|p| *p).collect();
        fpaths.sort();
        for path in fpaths.iter() {
            let entry = &filetree[path];
            match entry {
                &F(content) => {
                    let mut f = t!(fs::File::create(path));
                    t!(f.write_all(content.as_bytes()))
                },
                &D => {
                    t!(fs::create_dir(path))
                },
            }
        }

        for (inname, infilelist) in infilelists.iter() {
            let infile = t!(fs::File::create(inname));
            let inar = Archive::new(infile);
            for fname in infilelist {
                t!(inar.append_path(fname));
            }
            t!(inar.finish());
        }

        let mut infilenames: Vec<_> = infilelists.keys().map(|s| *s).collect();
        infilenames.sort();
        commonise_tars(&infilenames[..]);

        for (outname, outfilelist) in outfilelists.iter() {
            let outfile = t!(fs::File::open(outname));
            let outar = Archive::new(outfile);
            assert!(outfilelist.len() == t!(outar.files()).count());
            let acutalfilesiter = t!(outar.files()).map(|rf| t!(rf));
            for (expectedpathstr, actualfile) in outfilelist.iter().zip(acutalfilesiter) {
                let actualpath = t!(actualfile.header().path()).to_path_buf();
                assert!(expectedpathstr == &actualpath.to_str().unwrap());
            }
        }
    }
}
