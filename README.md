Crate to get CDDB information from gnudb.org (like cddb.com and freedb.org in the past).

It uses the discid crate to query the discid from the CDROM/DVDROM drive.

Right now only login, query and read are implemented, and only over CDDBP (not HTTP).

The code is now fully async.

Usage:

```Rust
// get a disc id by querying the disc in the default CD/DVD ROM drive
let discid = DiscId::read(Some(DiscId::default_device().as_str())).unwrap();
// open a connection
let mut con = Connection::new().await.unwrap();
// find a list of matches (could be multiple)
let matches: Vec<Match> = con.query(&discid).await.unwrap();
// select the right match
let ref m: Match = matches[2];
// read all the metadata
let _disc = con.read(&m).await.unwrap();
// close the connection (Drop trait is implemented, so not strictly necessary)
con.close();
```