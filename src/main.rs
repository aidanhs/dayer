// For test decorators
#![feature(plugin, custom_attribute)]
#![plugin(adorn)]
#![plugin(docopt_macros)]

// Literal maps for test purposes
#[cfg(test)]
#[macro_use] extern crate maplit;
#[cfg(test)]
#[macro_use] extern crate lazy_static;

extern crate docopt;
extern crate env_logger;
extern crate mime;
extern crate reqwest;
extern crate rustc_serialize;
extern crate tar;

mod util;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::{BufReader, BufWriter};
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str;

use reqwest::Client;
use reqwest::Response;
use reqwest::header::{Accept, Authorization, Bearer, Headers, qitem};
use reqwest::Method;
use mime::{Mime, TopLevel, SubLevel};
use reqwest::StatusCode;
use reqwest::Url;

use rustc_serialize::json;

use tar::Archive;

use util::{find_common_keys, format_num_bytes, readers_identical, to_string_slices};

// https://github.com/rust-lang/rust/issues/13721
#[derive(Clone)]
struct HashableHeader(tar::Header);
impl HashableHeader {
    pub fn new(srcheader: &tar::Header) -> HashableHeader {
        HashableHeader(srcheader.clone())
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
fn truncate(slice: &[u8]) -> &[u8] {
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
        let (spacekvslice, restdata2) = restdata.split_at(size as usize - sizeslice.len());
        let kvslice = &spacekvslice[1..spacekvslice.len() - 1];
        let eqpos: usize = kvslice.iter().position(|c| *c == b'=').unwrap();
        let (key, eqval) = kvslice.split_at(eqpos);
        let val = &eqval[1..];
        assert!(outmap.insert(str::from_utf8(key).unwrap(), str::from_utf8(val).unwrap())
                      .is_none());
        data = restdata2
    }
    outmap
}

fn get_header_map<'a, 'b>(arfiles: &'a mut Vec<tar::Entry<'b, fs::File>>)
                          -> HashMap<HashableHeader, &'a mut tar::Entry<'b, fs::File>> {
    let mut arfilemap: HashMap<HashableHeader, &'a mut tar::Entry<'b, _>> = HashMap::new();
    for file in arfiles.iter_mut() {
        arfilemap.insert(HashableHeader::new(file.header()), file);
    }
    arfilemap
}

fn make_layer_tar<'a,
                  'b: 'a,
                  I1: Iterator<Item = &'a HashableHeader>,
                  I2: Iterator<Item = &'a mut tar::Entry<'b, fs::File>>,
                  F: Fn(&Path) -> tar::Header>
    (outname: &str,
     headeriter: I1,
     verbatimiter: I2,
     headertofilemap: &mut HashMap<HashableHeader, &mut tar::Entry<fs::File>>,
     mkdir: F) {

    let outfile = fs::File::create(outname).unwrap();
    let outar = Archive::new(outfile);

    // Alphabetical ordering, lets us make assumptions about directory traversal
    let mut headers: Vec<&HashableHeader> = headeriter.collect();
    headers.sort_by(|h1, h2| h1.0.path_bytes().cmp(&h2.0.path_bytes()));

    let mut lastdir = PathBuf::new();
    // TODO: set trailing slash of dirs for belt and braces?
    for hheader in &headers {
        let header = &hheader.0;
        assert!(&header.ustar[..5] == b"ustar"); // TODO: get this as public?
        let path = header.path().unwrap();
        // Climb up to find common prefix
        while !path.starts_with(&lastdir) {
            lastdir = lastdir.parent().unwrap().to_path_buf();
        }
        // Climb down creating dirs as necessary
        let relpath = path.parent().unwrap().strip_prefix(&lastdir).unwrap().to_path_buf();
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

fn get_archive_entries<'a>(ar: &'a Archive<fs::File>,
                           tname: &str)
                           -> (Vec<tar::Entry<'a, fs::File>>, Vec<tar::Entry<'a, fs::File>>) {
    println!("Loading {}", tname);
    let mut ignoredfiles: Vec<tar::Entry<_>> = vec![];
    // Can't handle extended headers at the moment - skip the next block if
    // prefixed by an extended header
    let mut skipnext = false;
    let mut extpath = PathBuf::new();
    let emptypath = PathBuf::new();
    // If we've skipped directories because of an extended header, exclude
    // anything under that
    let mut skipdirs: HashSet<PathBuf> = HashSet::new();
    let arfiles: Vec<_> = ar.entries().unwrap().filter_map(|res| {
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
                let afm = &mut *(&af as *const tar::Entry<fs::File> as *mut tar::Entry<fs::File>);
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
             tname,
             arfiles.len(),
             ignoredfiles.len());
    (arfiles, ignoredfiles)
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

//       dayer export-image <imagetar>
docopt!(Args derive Debug, "
Usage:
       dayer commonise-tar <tarpath> <tarpath> [<tarpath>...]
       dayer download-image <imageurl> <targetdir>
       dayer --help

Options:
    --help     Show this message.
    <imageurl> A fully qualified image url (e.g. `ubuntu` would be specified as
               `https://registry-1.docker.io/library/ubuntu:latest`)
");

fn main() {
    let args: Args = Args::docopt().decode().unwrap_or_else(|e| e.exit());
    if args.cmd_commonise_tar {
        commonise_tars(&to_string_slices(&args.arg_tarpath))
    } else if args.cmd_download_image {
        download_image(&args.arg_imageurl, &args.arg_targetdir)
    } else {
        unreachable!("no cmd")
    }
}

pub fn commonise_tars(tnames: &[&str]) {
    println!("Opening tars");
    let ars: Vec<tar::Archive<_>> = tnames.iter()
                                          .map(|tname| {
                                              let file = fs::File::open(tname).unwrap();
                                              Archive::new(file)
                                          })
                                          .collect();
    let mut arfiless: Vec<Vec<tar::Entry<_>>> = vec![];
    let mut ignoredfiless: Vec<Vec<tar::Entry<_>>> = vec![];
    for (ar, tname) in ars.iter().zip(tnames) {
        let (arfiles, ignoredfiles) = get_archive_entries(ar, tname);
        arfiless.push(arfiles);
        ignoredfiless.push(ignoredfiles);
    }

    println!("Phase 1: metadata compare");
    let mut arheadmaps: Vec<HashMap<HashableHeader, &mut tar::Entry<_>>> =
        arfiless.iter_mut().map(|arfiles| get_header_map(arfiles)).collect();
    // ideally would be &HashableHeader, but that borrows the maps as immutable
    // which then conflicts with the mutable borrow later because a borrow of
    // either keys or values applies to the whole hashmap
    // https://github.com/rust-lang/rfcs/issues/1215
    let commonheaders: Vec<HashableHeader> = find_common_keys(&arheadmaps);
    let p1commonsize = commonheaders.iter().fold(0, |sum, h| sum + h.0.size().unwrap());
    let p1commonsizestr = format_num_bytes(p1commonsize);
    println!("Phase 1 complete: possible {} files with {}",
             commonheaders.len(),
             p1commonsizestr);

    println!("Phase 2: data compare");
    let mut commonfiles: Vec<HashableHeader> = vec![];
    // TODO: sort by offset in archive? means not seeking backwards
    for (i, hheader) in commonheaders.iter().enumerate() {
        let mut files: Vec<&mut tar::Entry<_>> = arheadmaps.iter_mut()
                                                           .map(|arhm| {
                                                               &mut **arhm.get_mut(hheader).unwrap()
                                                           })
                                                           .collect();
        // Do the files have the same contents?
        // Note we've verified they have the same size by now
        if readers_identical(&mut files) {
            commonfiles.push(hheader.clone());
        }
        if i % 100 == 0 {
            print!("    Done {}\r", i);
            io::stdout().flush().unwrap();
        }
        // Reset the file - each entry keeps track of its own position
        for f in &mut files {
            f.seek(io::SeekFrom::Start(0)).unwrap();
        }
    }
    let p2commonsize = commonfiles.iter().fold(0, |sum, h| sum + h.0.size().unwrap());
    let p2commonsizestr = format_num_bytes(p2commonsize);
    println!("Phase 2 complete: actual {} files with {}",
             commonfiles.len(),
             p2commonsizestr);

    println!("Phase 3a: preparing for layer creation");
    let tonormpath = |h: &HashableHeader| {
        // Normalise it https://github.com/rust-lang/rust/issues/29008
        h.0.path().unwrap().components().as_path().to_path_buf()
    };
    let commonmap: HashMap<PathBuf, &HashableHeader> = commonfiles.iter()
                                                                  .map(|h| (tonormpath(h), h))
                                                                  .collect();
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
    make_layer_tar(outname,
                   commonfiles.iter(),
                   vec![].iter_mut(),
                   arheadmaps.get_mut(0).unwrap(),
                   &minimalmkdir);
    println!("Phase 3b complete: created {}", outname);

    println!("Phase 3c: individual layer creation");
    let thievingmkdir = |dirpath: &Path| commonmap[dirpath].clone().0;
    for (i, (arheadmap, ignoredfiles)) in arheadmaps.iter_mut()
                                                    .zip(ignoredfiless.iter_mut())
                                                    .enumerate() {
        let outname = format!("individual_{}.tar", i);
        let outheads: Vec<_> = arheadmap.keys()
                                        .filter(|h| !commonmap.contains_key(&tonormpath(h)))
                                        .cloned()
                                        .collect();
        make_layer_tar(&outname,
                       outheads.iter(),
                       ignoredfiles.iter_mut(),
                       arheadmap,
                       &thievingmkdir);
    }
    println!("Phase 3c complete: created {} individual tars",
             arheadmaps.len());
}

fn req_maybe_bearer_auth(client: &Client, method: Method, url: Url, headers: Headers) -> Response {
    let res = client.request(Method::Get, url.clone()).headers(headers.clone()).send().unwrap();
    if *res.status() != StatusCode::Unauthorized {
        return res
    }
    let auth_challenge = res.headers().get_raw("www-authenticate").unwrap();
    assert!(auth_challenge.len() == 1);
    let mut auth_challenge = &auth_challenge[0][..];
    assert!(auth_challenge.starts_with(b"Bearer "));
    auth_challenge = &auth_challenge[b"Bearer ".len()..];
    let mut auth_challenge_realm = None;
    let mut auth_challenge_service = None;
    let mut auth_challenge_scope = None;
    loop {
        let eqpos = auth_challenge.iter().position(|&b| b == b'=').unwrap();
        let key = &auth_challenge[..eqpos];
        assert!(auth_challenge[eqpos + 1] == b'"');
        let valstart = eqpos + 2;
        let valend = valstart + auth_challenge.iter().skip(valstart).position(|&b| b == b'"').unwrap();
        let val = String::from_utf8(auth_challenge[valstart..valend].to_vec()).unwrap();
        match key {
            b"realm" => auth_challenge_realm = Some(val),
            b"service" => auth_challenge_service = Some(val),
            b"scope" => auth_challenge_scope = Some(val),
            _ => panic!(format!("unknown key in auth challenge {:?}", key)),
        }
        if auth_challenge.len() == valend + 1 { break }
        assert!(auth_challenge[valend + 1] == b',');
        auth_challenge = &auth_challenge[valend + 2..];
    }
    let mut authurl = Url::parse(&auth_challenge_realm.unwrap()).unwrap();
    authurl.query_pairs_mut()
        .append_pair("service", &auth_challenge_service.unwrap())
        .append_pair("scope", &auth_challenge_scope.unwrap());
    let authreq = client.request(Method::Get, authurl);
    let mut authjson = String::new();
    authreq.send().unwrap().read_to_string(&mut authjson).unwrap();
    #[derive(RustcDecodable)]
    struct AuthToken { token: String }
    let authtoken = json::decode::<AuthToken>(&authjson).unwrap().token;
    let newreq = client.request(method, url).headers(headers).header(Authorization(Bearer { token: authtoken }));
    newreq.send().unwrap()
}

fn download_image(imageurlstr: &str, targetdir: &str) {
    env_logger::init().unwrap();
    let imageurl = Url::parse(imageurlstr).unwrap();
    let imagename = imageurl.path();
    assert!(&imagename[0..1] == "/");
    let imagetagstart = imagename.bytes().position(|b| b == b':').unwrap();
    let imagetag = &imagename[imagetagstart+1..];
    let imagename = &imagename[1..imagetagstart];
    let mut registryurl = imageurl.clone();
    registryurl.set_path("v2/");

    fs::create_dir(targetdir).unwrap();

    let client = &Client::new().unwrap();

    let url = registryurl.join(&format!("{}/manifests/{}", imagename, imagetag)).unwrap();
    let mut manifestheaders = Headers::new();
    manifestheaders.set(Accept(vec![
        qitem(Mime(TopLevel::Application, SubLevel::Ext("vnd.docker.distribution.manifest.v2+json".to_owned()), vec![])),
    ]));
    let mut manifestjson = String::new();
    req_maybe_bearer_auth(client, Method::Get, url, manifestheaders).read_to_string(&mut manifestjson).unwrap();
    // https://docs.docker.com/registry/spec/api/#/pulling-an-image
    // https://docs.docker.com/registry/spec/manifest-v2-2/
    // Should really verify manifest
    #[derive(RustcDecodable)]
    struct Layer { digest: String }
    #[derive(RustcDecodable)]
    struct ImageManifest {
        layers: Vec<Layer>,
    }
    let manifest: ImageManifest = json::decode(&manifestjson).unwrap();
    let blobs: Vec<&str> = manifest.layers.iter().map(|fl| fl.digest.as_str()).collect();
    println!("Found {} blobs", blobs.len());
    let mut blobheaders = Headers::new();
    blobheaders.set(Accept(vec![
        qitem(Mime(TopLevel::Application, SubLevel::Ext("vnd.docker.image.rootfs.diff.tar.gzip".to_owned()), vec![])),
    ]));
    for blob in &blobs {
        println!("Downloading blob {}", blob);
        let file = File::create(blob).unwrap();
        let bloburl = registryurl.join(&format!("{}/blobs/{}", imagename, blob)).unwrap();
        let res = req_maybe_bearer_auth(client, Method::Get, bloburl, blobheaders.clone());
        io::copy(&mut BufReader::new(res), &mut BufWriter::new(file)).unwrap();
    }
    for blob in blobs.iter() {
        println!("Extracting blob {}", blob);
        let exit = Command::new("tar").args(&["--anchored", "--exclude=dev/*", "--force-local", "-C", targetdir, "-xf", blob])
            .spawn().unwrap().wait().unwrap();
        assert!(exit.success());
        let output = Command::new("find").args(&[targetdir, "-type", "f", "-name", ".wh.*", "-print0"]).output().unwrap();
        assert!(output.status.success());
        let mut whfilesstr = output.stdout;
        if whfilesstr.is_empty() { continue }
        assert!(*whfilesstr.last().unwrap() == b'\0');
        whfilesstr.pop();
        let whfiles: Vec<&str> = whfilesstr.split(|&b| b == b'\0').map(|bs| str::from_utf8(bs).unwrap()).collect();
        for whfile in whfiles {
            let whpath = Path::new(whfile);
            let whname = whpath.file_name().unwrap().to_str().unwrap();
            assert!(whname.starts_with(".wh."));
            let fname = &whname[4..];
            let mut fpath = whpath.parent().unwrap().to_path_buf();
            fpath.push(fname);
            if fpath.is_file() {
                fs::remove_file(fpath).unwrap()
            } else {
                fs::remove_dir_all(fpath).unwrap()
            }
            fs::remove_file(whpath).unwrap()
        }
    }
    let mut blobs = blobs;
    blobs.sort();
    blobs.dedup();
    for blob in blobs {
        println!("Removing {}", blob);
        fs::remove_file(blob).unwrap()
    }
}

#[cfg(test)]
mod tests {
    extern crate tempdir;

    use std::collections::HashMap;
    use std::env::set_current_dir;
    use std::fs;
    use std::io::prelude::*;
    use std::sync::Mutex;

    use self::tempdir::TempDir;
    use self::DirTreeEntry::*;
    use super::tar::Archive;

    use super::commonise_tars;

    macro_rules! t {
        ($e:expr) => (match $e {
            Ok(v) => v,
            Err(e) => panic!("{} returned {}", stringify!($e), e),
        })
    }

    lazy_static! {
        pub static ref TMPLOCK: Mutex<()> = Mutex::new(());
    }

    // Does not put program back in original dir
    fn intmp<F>(f: F)
        where F: Fn()
    {
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
                }
                &D => t!(fs::create_dir(path)),
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
            assert!(outfilelist.len() == t!(outar.entries()).count());
            let acutalfilesiter = t!(outar.entries()).map(|rf| t!(rf));
            for (expectedpathstr, actualfile) in outfilelist.iter().zip(acutalfilesiter) {
                let actualpath = t!(actualfile.header().path()).to_path_buf();
                assert!(expectedpathstr == &actualpath.to_str().unwrap());
            }
        }
    }
}
