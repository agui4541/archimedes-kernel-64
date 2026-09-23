use hole_punch::*;

use std::env;
use std::fs::File;

fn main() -> Result<(), ScanError> {
    let args: Vec<String> = env::args().collect();
    assert!(args.len() > 1);
    println!("{}", args[1]);
    let mut file = File::open(&args[1])?;
    let chunks = file.scan_chunks()?;
    for chunk in chunks {
        println!("{:?}", chunk);
    }

    Ok(())
}
