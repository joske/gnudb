// Network/integration tests
// These use #[serial] because gnudb doesn't like multiple connections from same IP
// These tests are ignored by default to avoid network calls on CI

use discid::DiscId;
use log::debug;
use serial_test::serial;

use crate::{Connection, http_query, http_read};

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
#[ignore]
fn test_good_url() {
    init_logger();
    let con = aw!(Connection::from_host_port("gnudb.gnudb.org", 8880));
    assert!(con.is_ok());
}

#[test]
#[serial]
#[ignore]
fn test_bad_url() {
    init_logger();
    let con = aw!(Connection::from_host_port("localhost", 80));
    assert!(con.is_err());
}

#[test]
#[serial]
#[ignore]
fn test_http_exact_search() {
    init_logger();
    let offsets = [
        185_700, 150, 18_051, 42_248, 57_183, 75_952, 89_333, 114_384, 142_453, 163_641,
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
#[ignore]
fn test_exact_search() {
    init_logger();
    let offsets = [
        185_700, 150, 18_051, 42_248, 57_183, 75_952, 89_333, 114_384, 142_453, 163_641,
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
#[ignore]
fn test_inexact_search() {
    init_logger();
    let offsets = [
        185_710, 150, 18_025, 42_275, 57_184, 75_952, 89_333, 114_386, 142_451, 163_695,
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
