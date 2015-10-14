#![feature(path_relative_from)]

// For test decorators
#![feature(plugin, custom_attribute)]
#![plugin(adorn)]
// Only allow one test directory at a time
#![feature(static_mutex)]

extern crate tar;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
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

fn make_layer_tar<'a, I: Iterator<Item=&'a HashableHeader>, F: Fn(&Path) -> tar::Header>(
        outname: &str,
        headeriter: I,
        headertofilemap: &mut HashMap<HashableHeader, &mut tar::File<fs::File>>,
        mkdir: F) {

    let outfile = fs::File::create(outname).unwrap();
    // Can append even though it's not mutable
    // https://github.com/alexcrichton/tar-rs/issues/31
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
    if args.len() < 3 {
        println!("Invalid number of args: {}", args.len());
    }
    let slargs: Vec<&str> = args[1..].iter().map(|s| &s[..]).collect();
    commonise_tars(&slargs[..]);
}

pub fn commonise_tars(tnames: &[&str]) {
    let numars = tnames.len();

    println!("Opening tars");
    let ars: Vec<tar::Archive<fs::File>> = tnames.iter().map(|tname| {
        let file = fs::File::open(tname).unwrap();
        Archive::new(file)
    }).collect();
    let mut arfiless: Vec<Vec<tar::File<fs::File>>> = ars.iter().zip(tnames).map(|(ar, tname)| {
        println!("Loading {}", tname);
        let mut skipnext = false;
        let arfiles: Vec<_> = ar.files().unwrap().filter_map(|res| {
            let af = res.unwrap();
            match af.header().link[0] {
              b'g' => panic!("Cannot handle global extended header"),
              b'x' => panic!("Cannot handle extended header"),
              _ if skipnext => { skipnext = false; None },
              _ => Some(af),
            }
        }).collect();
        println!("Loading {}: found {} files", tname, arfiles.len());
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

    println!("Phase 3: common layer creation");
    // Create a holding-place directory for the common layer as it will be
    // by the layer above
    let minimalmkdir = |dirpath: &Path| {
        let mut newdir = tar::Header::new();
        newdir.set_path(&dirpath).unwrap();
        newdir.set_mode(0);
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
    make_layer_tar(outname, p2result.iter(), arheadmaps.get_mut(0).unwrap(), &minimalmkdir);
    println!("Phase 3 complete: created {}", outname);

    println!("Phase 4: individual layer creation");
    let tonormpath = |h: &HashableHeader| {
        h.0.path().unwrap().components().as_path().to_path_buf()
    };
    let commonmap: HashMap<PathBuf, &HashableHeader> = p2result
        .iter().map(|h| (tonormpath(h), h)).collect();
    let thievingmkdir = |dirpath: &Path| {
        commonmap[dirpath].clone().0
    };
    for (i, arheadmap) in arheadmaps.iter_mut().enumerate() {
      let outname = format!("individual_{}.tar", i);
      let outheads: Vec<_> = arheadmap
          .keys().filter(|h| !commonmap.contains_key(&tonormpath(h))).map(|h| h.clone()).collect();
      make_layer_tar(&outname, outheads.iter(), arheadmap, &thievingmkdir);
    }
    println!("Phase 4 complete: created {} individual tars", arheadmaps.len());
}

#[cfg(test)]
mod tests {
    extern crate tempdir;

    use std::env::{current_dir, set_current_dir};
    use std::fs;
    use std::io::prelude::*;
    use std::sync::{StaticMutex, MUTEX_INIT};

    use self::tempdir::TempDir;
    use super::tar::Archive;

    use super::*;

    macro_rules! t {
        ($e:expr) => (match $e {
            Ok(v) => v,
            Err(e) => panic!("{} returned {}", stringify!($e), e),
        })
    }

    static TMPLOCK: StaticMutex = MUTEX_INIT;

    fn intmp<F>(f: F) where F: Fn() {
        let _g = TMPLOCK.lock().unwrap(); // destroyed at end of fn
        let td = TempDir::new("dayer").unwrap(); // destroyed at end of fn
        let old = current_dir().unwrap();
        set_current_dir(td.path()).unwrap();
        f();
        set_current_dir(old).unwrap();
    }

    #[test]
    #[adorn(intmp)]
    fn empty_tars() {
        let innames = vec!["in0.tar", "in1.tar"];
        for inname in &innames[..] {
            let infile = t!(fs::File::create(inname));
            let inar = Archive::new(infile);
            t!(inar.finish());
        }

        commonise_tars(&innames[..]);

        let outnames = vec!["common.tar", "individual_0.tar", "individual_1.tar"];
        for outname in &outnames[..] {
            let outfile = t!(fs::File::open(outname));
            let outar = Archive::new(outfile);
            assert!(t!(outar.files()).count() == 0);
        }
    }

    #[test]
    #[adorn(intmp)]
    fn simple_tars() {
        let fnames = vec!["0", "1", "common"];
        for fname in &fnames[..] {
            let mut f = t!(fs::File::create(fname));
            t!(f.write_all(fname.as_bytes()));
        }

        let innames = vec!["in1.tar", "in2.tar"];
        for (i, inname) in (&innames[..]).iter().enumerate() {
            let infile = t!(fs::File::create(inname));
            let inar = Archive::new(infile);
            t!(inar.append_path(format!("{}", i)));
            t!(inar.append_path("common"));
            t!(inar.finish());
        }

        commonise_tars(&innames[..]);

        let outnames = vec!["common.tar", "individual_0.tar", "individual_1.tar"];
        for outname in &outnames[..] {
            let outfile = t!(fs::File::open(outname));
            let outar = Archive::new(outfile);
            assert!(t!(outar.files()).count() == 1);
        }
    }
}
