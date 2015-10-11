extern crate tar;

use std::env;
use std::io::prelude::*;
use std::io::SeekFrom;
use std::fs::File;
use std::path::{Path, PathBuf};
use tar::Archive;

fn get_initial_filelist(a: &Archive<File>) -> Vec<(PathBuf, u64)> {
    let mut afiles: Vec<(PathBuf, u64)> = vec![];
    for file in a.files().unwrap() {
        // Make sure there wasn't an I/O error
        let mut file = file.unwrap();

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

    println!("Loading {}", tname2);
    let file2 = File::open(tname2).unwrap();
    let ar2 = Archive::new(file2);
    let afiles2 = get_initial_filelist(&ar2);

}
