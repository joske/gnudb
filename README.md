Crate to get CDDB information from gnudb.org (like cddb.com and freedb.org in the past).

It uses the discid crate to query the discid from the CDROM/DVDROM drive.

Right now only login, query and read are implemented, and only over CDDBP (not HTTP).

Usage:

```Rust
// get a disc id by querying the disc in the default CD/DVD ROM drive
let discid = DiscId::read(Some(DiscId::default_device().as_str())).unwrap();
// open a connection
let mut con = gnudb::Connection::new().unwrap();
// find a list of matches (could be multiple)
let matches : Vec<Match> = con.query(&discid).unwrap();
// select the right match
let m : Match = matches[2];
// read all the metadata
let disc = con.read(&m).unwrap();
// close the connection (Drop trait is implemented, so not strictly necessary)
con.close();
```