Crate to get CDDB information from gnudb.org (like cddb.com and freedb.org in the past).

It uses the discid crate to query the discid from the CDROM/DVDROM drive.

Right now only login, query and read are implemented, and only over CDDBP (not HTTP).

Usage:
let discid = DiscId::read(Some(DiscId::default_device().as_str())).unwrap();
let disc = gnudb::gnudb(&discid).unwrap();