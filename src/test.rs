// Network/integration tests
// These use #[serial] because gnudb doesn't like multiple connections from same IP

use discid::DiscId;
use log::debug;
use serial_test::serial;

use crate::{http_query, http_read, Connection};

macro_rules! aw {
    ($e:expr) => {
        smol::block_on($e)
    };
}

fn init_logger() {
    let _ = env_logger::builder().is_test(true).try_init();
}

#[test]
#[serial]
fn test_good_url() {
    init_logger();
    let con = aw!(Connection::from_host_port("gnudb.gnudb.org", 8880));
    assert!(con.is_ok());
}

#[test]
#[serial]
fn test_bad_url() {
    init_logger();
    let con = aw!(Connection::from_host_port("localhost", 80));
    assert!(con.is_err());
}

#[test]
#[serial]
fn test_http_exact_search() {
    init_logger();
    let offsets = [
        185700, 150, 18051, 42248, 57183, 75952, 89333, 114384, 142453, 163641,
    ];
    let discid = DiscId::put(1, &offsets).unwrap();
    debug!("freedb {}", discid.freedb_id());
    let matches = http_query("gnudb.gnudb.org", 80, &discid);
    assert!(matches.is_ok());
    let matches = matches.unwrap();
    assert_eq!(matches.len(), 1);
    let disc = http_read("gnudb.gnudb.org", 80, &matches[0]);
    assert!(disc.is_ok());
    let disc = disc.unwrap();
    assert_eq!(disc.year.unwrap(), 1978);
    assert_eq!(disc.tracks.len(), 9);
    assert_eq!(disc.genre.unwrap(), "Rock");
    assert_eq!(disc.title, "Dire Straits");
    assert_eq!(disc.artist, "DIRE STRAITS");
}

#[test]
#[serial]
fn test_exact_search() {
    init_logger();
    let offsets = [
        185700, 150, 18051, 42248, 57183, 75952, 89333, 114384, 142453, 163641,
    ];
    let discid = DiscId::put(1, &offsets).unwrap();
    debug!("freedb {}", discid.freedb_id());
    let mut con = aw!(Connection::new()).unwrap();
    let matches = aw!(con.query(&discid));
    assert!(matches.is_ok());
    let matches = matches.unwrap();
    assert_eq!(matches.len(), 1);
    let disc = aw!(con.read(&matches[0]));
    assert!(disc.is_ok());
    let disc = disc.unwrap();
    assert_eq!(disc.year.unwrap(), 1978);
    assert_eq!(disc.tracks.len(), 9);
    assert_eq!(disc.genre.unwrap(), "Rock");
    assert_eq!(disc.title, "Dire Straits");
    assert_eq!(disc.artist, "DIRE STRAITS");
}

#[test]
#[serial]
fn test_inexact_search() {
    init_logger();
    let offsets = [
        185710, 150, 18025, 42275, 57184, 75952, 89333, 114386, 142451, 163695,
    ];
    let discid = DiscId::put(1, &offsets).unwrap();
    debug!("freedb {}", discid.freedb_id());
    let mut con = aw!(Connection::new()).unwrap();
    let matches = aw!(con.query(&discid));
    assert!(matches.is_ok());
    let matches = matches.unwrap();
    assert_eq!(matches.len(), 1);
    let disc = aw!(con.read(&matches[0]));
    assert!(disc.is_ok());
    let disc = disc.unwrap();
    assert_eq!(disc.year.unwrap(), 1978);
    assert_eq!(disc.tracks.len(), 9);
    assert_eq!(disc.genre.unwrap(), "Rock");
    assert_eq!(disc.title, "Dire Straits");
    assert_eq!(disc.artist, "DIRE STRAITS");
}
