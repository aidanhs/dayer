extern crate tar;

use std::env;
use std::fs::File;
use std::path::PathBuf;
use tar::Archive;

fn get_initial_filelist(a: &Archive<File>) -> Vec<(PathBuf, u64)> {
    let mut afiles: Vec<(PathBuf, u64)> = vec![];
    for file in a.files().unwrap() {
        // Make sure there wasn't an I/O error
        let file = file.unwrap();

        // Get basic file metadata
        let header = file.header();
        let path = header.path().unwrap().into_owned();
        let size = header.size().unwrap();

        if size == 0 { continue }

        afiles.push((path, size));
    }
    afiles.sort_by(|a, b| a.0.cmp(&b.0));
    afiles
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
    let len1 = afiles1.len();
    println!("Loading {}: found {} files", tname1, afiles1.len());

    println!("Loading {}", tname2);
    let file2 = File::open(tname2).unwrap();
    let ar2 = Archive::new(file2);
    let afiles2 = get_initial_filelist(&ar2);
    let len2 = afiles2.len();
    println!("Loading {}: found {} files", tname2, afiles2.len());

    println!("Phase 1 compare start");
    let mut p1same: Vec<PathBuf> = vec![];
    let mut p1size: u64 = 0;

    let mut idx1: usize = 0;
    let mut idx2: usize = 0;
    loop {
        let (ref name1, size1) = afiles1[idx1];
        let (ref name2, size2) = afiles2[idx2];
        if name1 < name2 {
            idx1 += 1;
        } else if name2 < name1 {
            idx2 += 1;
        } else {
            if size1 == size2 {
                p1same.push(name1.to_owned());
                p1size += size1;
            }
            idx1 += 1;
            idx2 += 1;
        }
        // Could be done in the conditionals but let's be honest, it's
        // not a bottleneck
        if idx1 == len1 || idx2 == len2 { break }
    }
    println!("Phase 1 compare end: {} files with {} bytes", p1same.len(), p1size);

    // prune dirs
}
