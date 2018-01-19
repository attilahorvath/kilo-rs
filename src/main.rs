extern crate kilo_rs;

use kilo_rs::*;

fn main() {
    let kilo = match Kilo::new() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("Unable to initialize Kilo: {}", e);
            return;
        }
    };

    if let Err(e) = kilo.run() {
        eprintln!("Error encountered while running Kilo: {}", e);
    }
}
