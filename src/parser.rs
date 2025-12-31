use discid::DiscId;
use log::debug;

use crate::error::GnuDbError;
use crate::{Disc, Match, Track};

pub(crate) fn create_query_cmd(discid: &DiscId) -> Result<String, GnuDbError> {
    let count = discid.last_track_num() - discid.first_track_num() + 1;
    let mut toc = discid.toc_string();
    let mut split = toc.splitn(4, ' ');
    toc = split
        .nth(3)
        .ok_or(GnuDbError::ProtocolError("failed to parse toc".to_string()))?
        .to_owned();
    let query = format!(
        "cddb query {} {} {} {}\n",
        discid.freedb_id(),
        count,
        toc,
        discid.sectors() / 75
    );
    Ok(query)
}

pub(crate) fn create_read_cmd(single_match: &Match) -> String {
    format!(
        "cddb read {} {}\n",
        single_match.category, single_match.discid
    )
}

/// parse the raw response from the server according to the protocol
pub(crate) fn parse_raw_response(raw: &str) -> Result<String, GnuDbError> {
    let (status, rest) = match raw.split_once('\n') {
        Some((head, tail)) => (head, Some(tail)),
        None => (raw, None),
    };

    if status.starts_with('4') || status.starts_with('5') {
        return Err(GnuDbError::ProtocolError(status.to_string()));
    }

    let second_digit = status.chars().nth(1).ok_or(GnuDbError::ProtocolError(
        "failed to parse response code".to_string(),
    ))?;
    if second_digit == '0' {
        if rest.is_some() {
            return Ok(format!("{status}\n"));
        }
        return Ok(status.to_string());
    }

    if second_digit != '1' && second_digit != '2' {
        return Err(GnuDbError::ProtocolError(status.to_string()));
    }

    let mut data = String::new();
    if let Some(rest) = rest {
        for line in rest.lines() {
            if line == "." {
                break;
            }
            if let Some(stripped) = line.strip_prefix("..") {
                data.push('.');
                data.push_str(stripped);
            } else {
                data.push_str(line);
            }
            data.push('\n');
        }
    }

    Ok(data)
}

pub(crate) fn parse_query_response(response: &str) -> Result<Vec<Match>, GnuDbError> {
    let mut matches: Vec<Match> = Vec::new();
    for line in response.lines() {
        if line.starts_with("200") {
            // exact match
            let mut split = line.splitn(4, ' ');
            let _code = split.next();
            let category = split.next().ok_or(GnuDbError::ProtocolError(
                "failed to parse exact match category".to_owned(),
            ))?;
            let discid = split.next().ok_or(GnuDbError::ProtocolError(
                "failed to parse exact match discid".to_owned(),
            ))?;
            let remainder = split.next().ok_or(GnuDbError::ProtocolError(
                "failed to parse exact match remainder".to_owned(),
            ))?;
            let (artist, title) = split_artist_title(remainder)?;
            let m = Match {
                discid: discid.to_owned(),
                category: category.to_owned(),
                title,
                artist,
            };
            matches.push(m);
            break;
        }
        if line.starts_with("202") {
            // no matches
            break;
        }
        if line.starts_with("211") {
            continue; // ignore first status line
        }
        let m = parse_matches(line)?;
        matches.push(m);
    }
    Ok(matches)
}

/// parse a line of inexact matches
pub(crate) fn parse_matches(line: &str) -> Result<Match, GnuDbError> {
    let mut split = line.splitn(3, ' ');
    let category = split.next().ok_or(GnuDbError::ProtocolError(
        "failed to parse category".to_owned(),
    ))?;
    let id = split
        .next()
        .ok_or(GnuDbError::ProtocolError("failed to parse id".to_owned()))?;
    let remainder = split.next().ok_or(GnuDbError::ProtocolError(
        "failed to parse remainder".to_owned(),
    ))?;
    let (artist, title) = split_artist_title(remainder)?;
    Ok(Match {
        discid: id.to_owned(),
        category: category.to_owned(),
        title,
        artist,
    })
}

fn split_artist_title(remainder: &str) -> Result<(String, String), GnuDbError> {
    let (artist, title) = remainder
        .split_once(" / ")
        .or_else(|| remainder.split_once('/'))
        .ok_or(GnuDbError::ProtocolError(
            "failed to parse artist/title".to_string(),
        ))?;
    Ok((artist.trim().to_owned(), title.trim().to_owned()))
}

/// parse the full response from the CDDB server
pub(crate) fn parse_read_response(data: &str) -> Result<Disc, GnuDbError> {
    debug!("{data}");
    let mut disc = Disc {
        ..Default::default()
    };
    for line in data.lines() {
        if let Some(value) = line.strip_prefix("DTITLE=") {
            let mut split = value.splitn(2, '/');
            let first = split.next().unwrap_or("").trim();
            if let Some(rest) = split.next() {
                first.clone_into(&mut disc.artist);
                rest.trim().clone_into(&mut disc.title);
            } else {
                first.clone_into(&mut disc.title);
            }
        }
        if let Some(value) = line.strip_prefix("DYEAR=") {
            let value = value.trim();
            if !value.is_empty() {
                match value.parse::<u16>() {
                    Ok(year) => disc.year = Some(year),
                    Err(e) => {
                        debug!("failed to parse DYEAR '{value}': {e}");
                    }
                }
            }
        }
        if let Some(value) = line.strip_prefix("DGENRE=") {
            let value = value.trim();
            if !value.is_empty() {
                disc.genre = Some(value.to_owned());
            }
        }
        // since we use protocol level 6, we should get the year/genre via DYEAR and DGENRE, and these should come before EXTD
        // this is as a fallback
        if disc.year.is_none()
            && line.starts_with("EXTD")
            && let Some(pos) = line.find("YEAR:")
        {
            let value = line[(pos + "YEAR:".len())..]
                .split_whitespace()
                .next()
                .ok_or(GnuDbError::ProtocolError(
                    "failed to parse EXTD YEAR".to_owned(),
                ))?;
            disc.year = Some(value.parse::<u16>().map_err(|e| {
                GnuDbError::ProtocolError(format!("failed to parse EXTD YEAR: {e}"))
            })?);
        }

        if line.starts_with("TTITLE") {
            let rest = line
                .strip_prefix("TTITLE")
                .ok_or(GnuDbError::ProtocolError(
                    "failed to parse TTITLE".to_owned(),
                ))?;
            let (index_str, title) = rest.split_once('=').ok_or(GnuDbError::ProtocolError(
                "failed to parse TTITLE value".to_owned(),
            ))?;
            let index = index_str.trim().parse::<u32>().map_err(|e| {
                GnuDbError::ProtocolError(format!("failed to parse TTITLE index: {e}"))
            })?;
            let mut track = Track {
                ..Default::default()
            };
            track.number = index + 1; // tracks are 0 based in CDDB/GNUDB
            title.clone_into(&mut track.title);
            track.artist.clone_from(&disc.artist);
            disc.tracks.push(track);
        }
    }
    Ok(disc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_logger() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn test_parse_response_multiline_dotstuff() -> Result<(), GnuDbError> {
        let raw = "211 multiple matches\n..hello\nworld\n.\n";
        let data = parse_raw_response(raw)?;
        assert_eq!(data, ".hello\nworld\n");
        Ok(())
    }

    #[test]
    fn test_parse_response_single_line() -> Result<(), GnuDbError> {
        let raw = "200 OK\n";
        let data = parse_raw_response(raw)?;
        assert_eq!(data, "200 OK\n");
        Ok(())
    }

    #[test]
    fn test_parse_response_error() {
        let raw = "500 fail\n";
        let err = parse_raw_response(raw).unwrap_err();
        match err {
            GnuDbError::ProtocolError(_) => {}
            GnuDbError::ConnectionError(_) => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_parse_response_4xx_error() {
        let raw = "401 Permission denied\n";
        let err = parse_raw_response(raw).unwrap_err();
        match err {
            GnuDbError::ProtocolError(msg) => assert!(msg.contains("401")),
            GnuDbError::ConnectionError(_) => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_parse_response_3xx_error() {
        // 3xx with second digit not 0/1/2 should error
        let raw = "330 connection closing\n";
        let err = parse_raw_response(raw).unwrap_err();
        match err {
            GnuDbError::ProtocolError(_) => {}
            GnuDbError::ConnectionError(_) => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_parse_response_no_newline() -> Result<(), GnuDbError> {
        let raw = "200 OK";
        let data = parse_raw_response(raw)?;
        assert_eq!(data, "200 OK");
        Ok(())
    }

    #[test]
    fn test_dotstuff_multiple_dots() -> Result<(), GnuDbError> {
        let raw = "210 data\n...\n...test\n.\n";
        let data = parse_raw_response(raw)?;
        assert_eq!(data, "..\n..test\n");
        Ok(())
    }

    #[test]
    fn test_empty_response_body() -> Result<(), GnuDbError> {
        let raw = "210 data\n.\n";
        let data = parse_raw_response(raw)?;
        assert_eq!(data, "");
        Ok(())
    }

    #[test]
    fn test_no_match() {
        init_logger();
        let matches = parse_query_response("202 No match for disc ID 000c4804.");
        assert!(matches.is_ok());
        let matches = matches.unwrap();
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_parse_multiple_inexact_matches() -> Result<(), GnuDbError> {
        init_logger();
        let response = "211 Found inexact matches, list follows\n\
            rock abc123 Artist One / Album One\n\
            jazz def456 Artist Two / Album Two\n\
            blues ghi789 Artist Three / Album Three\n";
        let matches = parse_query_response(response)?;
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].category, "rock");
        assert_eq!(matches[0].discid, "abc123");
        assert_eq!(matches[0].artist, "Artist One");
        assert_eq!(matches[0].title, "Album One");
        assert_eq!(matches[1].category, "jazz");
        assert_eq!(matches[2].category, "blues");
        Ok(())
    }

    #[test]
    fn test_parse_exact_match_response() -> Result<(), GnuDbError> {
        init_logger();
        let response = "200 rock abc123 The Artist / The Album\n";
        let matches = parse_query_response(response)?;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].category, "rock");
        assert_eq!(matches[0].discid, "abc123");
        assert_eq!(matches[0].artist, "The Artist");
        assert_eq!(matches[0].title, "The Album");
        Ok(())
    }

    #[test]
    fn test_parse_matches_artist_title() -> Result<(), GnuDbError> {
        init_logger();
        let m = parse_matches("rock abc123 ARTIST / Recording Title")?;
        assert_eq!(m.artist, "ARTIST");
        assert_eq!(m.title, "Recording Title");
        assert_eq!(m.category, "rock");
        assert_eq!(m.discid, "abc123");
        Ok(())
    }

    #[test]
    fn test_parse_matches_with_slash_in_artist() -> Result<(), GnuDbError> {
        init_logger();
        // Parser splits on ` / ` (with spaces), so AC/DC works correctly
        let m = parse_matches("rock abc123 AC/DC / Back In Black")?;
        assert_eq!(m.artist, "AC/DC");
        assert_eq!(m.title, "Back In Black");
        Ok(())
    }

    #[test]
    fn test_parse_matches_with_slash_in_title() -> Result<(), GnuDbError> {
        init_logger();
        let m = parse_matches("rock abc123 Artist / Title/With/Slashes")?;
        assert_eq!(m.artist, "Artist");
        assert_eq!(m.title, "Title/With/Slashes");
        Ok(())
    }

    #[test]
    fn test_parse_matches_without_spaces() -> Result<(), GnuDbError> {
        init_logger();
        let m = parse_matches("rock abc123 Artist/Title")?;
        assert_eq!(m.artist, "Artist");
        assert_eq!(m.title, "Title");
        Ok(())
    }

    #[test]
    fn test_parse_matches_slash_without_spaces_in_title() -> Result<(), GnuDbError> {
        init_logger();
        let m = parse_matches("rock abc123 Artist/Title/With/Slashes")?;
        assert_eq!(m.artist, "Artist");
        assert_eq!(m.title, "Title/With/Slashes");
        Ok(())
    }

    #[test]
    fn test_parse_matches_missing_title() {
        let result = parse_matches("rock abc123 Artist Only");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_matches_empty_line() {
        let result = parse_matches("");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_read_cmd() {
        let m = Match {
            discid: "abc123".to_string(),
            category: "rock".to_string(),
            artist: "Artist".to_string(),
            title: "Title".to_string(),
        };
        let cmd = create_read_cmd(&m);
        assert_eq!(cmd, "cddb read rock abc123\n");
    }

    const RAMMSTEIN: &str = r"# xmcd
#
# Track frame offsets:
#    150
#    25075
#    46501
#    70596
#    88533
#    105910
#    125169
#    147365
#    162906
#    190441
#    215174
#
# Disc length: 3186 seconds
#
# Revision: 2
# Processed by: cddbd v1.5.1PL2 Copyright (c) Steve Scherf et al.
# Submitted via: audiograbber 1.83.01
#
DISCID=940c700b
DTITLE=Rammstein+Sixtynine / (black) Mutter
DYEAR=2002
DGENRE=Industrial Metal
TTITLE0=Mein Herz Brennt (Nun Liebe Kinder Mix)
TTITLE1=Links 234 (Zwei Drei Vier Mix)
TTITLE2=Sonne (Laut Bis Zehn Mix)
TTITLE3=Ich Will (Ich Will Mix)
TTITLE4=Feuer Frei (Bang Bang Mein Ungluck Mix)
TTITLE5=Mutter (Violin Mix)
TTITLE6=Spieluhr (Ein Kleiner Mensch  Mix)
TTITLE7=Zwitter (Zwitter Zwitter Mix)
TTITLE8=Rein Raus (Raus Motherfucker Rein Mix)
TTITLE9=Adios (Er Hat Die Augen Aufgemacht Mix)
TTITLE10=Nebel (Eng Umschlungen Mix)
EXTD=
EXTT0=
EXTT1=
EXTT2=
EXTT3=
EXTT4=
EXTT5=
EXTT6=
EXTT7=
EXTT8=
EXTT9=
EXTT10=
PLAYORDER=";

    const DIRE_STRAITS: &str = r"# xmcd
#
# Track frame offsets:
#    150
#    18051
#    42248
#    57183
#    75952
#    89333
#    114384
#    142453
#    163641
#
# Disc length: 2476 seconds
#
# Revision: 7
# Processed by: cddbd v1.4PL0 Copyright (c) Steve Scherf et al.
# Submitted via: EasyCDDAExtractor 5.1.0
#
DISCID=6909aa09
DTITLE=DIRE STRAITS / Dire Straits
DYEAR=1978
DGENRE=Rock
TTITLE0=Down to the waterline
TTITLE1=Water of love
TTITLE2=Setting me up
TTITLE3=Six blade knife
TTITLE4=Southbound again
TTITLE5=Sultans of swing
TTITLE6=In the gallery
TTITLE7=Wild west end
TTITLE8=Lions
EXTD= YEAR: 1978 ID3G: 17
EXTT0=
EXTT1=
EXTT2=
EXTT3=
EXTT4=
EXTT5=
EXTT6=
EXTT7=
EXTT8=
PLAYORDER=";

    #[test]
    fn test_parse() -> Result<(), GnuDbError> {
        init_logger();
        let disc = parse_read_response(RAMMSTEIN)?;
        assert_eq!(disc.year.unwrap(), 2002);
        assert_eq!(disc.title, "(black) Mutter");
        assert_eq!(disc.tracks.len(), 11);
        assert_eq!(disc.genre.unwrap(), "Industrial Metal");
        Ok(())
    }

    #[test]
    fn test_extd() -> Result<(), GnuDbError> {
        init_logger();
        let disc = parse_read_response(DIRE_STRAITS)?;
        assert_eq!(disc.year.unwrap(), 1978);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.title, "Dire Straits");
        assert_eq!(disc.artist, "DIRE STRAITS");
        Ok(())
    }

    #[test]
    fn test_missing_dyear() -> Result<(), GnuDbError> {
        init_logger();
        let data =
            "DTITLE=Unknown Artist / Mystery Record\nDYEAR=\nDGENRE=Unknown\nTTITLE0=Track 01\n";
        let disc = parse_read_response(data)?;
        assert!(disc.year.is_none());
        assert_eq!(disc.title, "Mystery Record");
        assert_eq!(disc.artist, "Unknown Artist");
        Ok(())
    }

    #[test]
    fn test_invalid_dyear_uses_extd() -> Result<(), GnuDbError> {
        init_logger();
        let data = "DTITLE=Sample Artist / Sample Title\nDYEAR=abcd\nDGENRE=Alt\nTTITLE0=Track 01\nEXTD= YEAR: 1999\n";
        let disc = parse_read_response(data)?;
        assert_eq!(disc.year, Some(1999));
        assert_eq!(disc.genre.as_deref(), Some("Alt"));
        Ok(())
    }

    #[test]
    fn test_valid_dyear_overrides_extd() -> Result<(), GnuDbError> {
        init_logger();
        let data =
            "DTITLE=Artist / Title\nDYEAR=2001\nDGENRE=Rock\nTTITLE0=Song\nEXTD= YEAR: 1980\n";
        let disc = parse_read_response(data)?;
        assert_eq!(disc.year, Some(2001));
        Ok(())
    }

    #[test]
    fn test_tracks_inherit_artist_and_numbering() -> Result<(), GnuDbError> {
        init_logger();
        let data = "DTITLE=Sample Artist / Example Album\nDYEAR=\nTTITLE0=Track Zero\nTTITLE1=Track One\nEXTD= YEAR: 1995\n";
        let disc = parse_read_response(data)?;
        assert_eq!(disc.artist, "Sample Artist");
        assert_eq!(disc.title, "Example Album");
        assert_eq!(disc.genre, None);
        assert_eq!(disc.year, Some(1995));
        assert_eq!(disc.tracks.len(), 2);
        assert_eq!(disc.tracks[0].number, 1);
        assert_eq!(disc.tracks[0].title, "Track Zero");
        assert_eq!(disc.tracks[0].artist, "Sample Artist");
        assert_eq!(disc.tracks[1].number, 2);
        assert_eq!(disc.tracks[1].title, "Track One");
        assert_eq!(disc.tracks[1].artist, "Sample Artist");
        Ok(())
    }

    #[test]
    fn test_dtitle_without_artist() -> Result<(), GnuDbError> {
        init_logger();
        let data = "DTITLE=Just A Title\nDYEAR=2000\nTTITLE0=Track\n";
        let disc = parse_read_response(data)?;
        assert_eq!(disc.title, "Just A Title");
        assert_eq!(disc.artist, "");
        Ok(())
    }

    #[test]
    fn test_empty_genre() -> Result<(), GnuDbError> {
        init_logger();
        let data = "DTITLE=Artist / Album\nDYEAR=2000\nDGENRE=\nTTITLE0=Track\n";
        let disc = parse_read_response(data)?;
        assert!(disc.genre.is_none());
        Ok(())
    }

    #[test]
    fn test_whitespace_only_genre() -> Result<(), GnuDbError> {
        init_logger();
        let data = "DTITLE=Artist / Album\nDYEAR=2000\nDGENRE=   \nTTITLE0=Track\n";
        let disc = parse_read_response(data)?;
        assert!(disc.genre.is_none());
        Ok(())
    }

    #[test]
    fn test_disc_with_no_tracks() -> Result<(), GnuDbError> {
        init_logger();
        let data = "DTITLE=Artist / Album\nDYEAR=2000\nDGENRE=Rock\n";
        let disc = parse_read_response(data)?;
        assert_eq!(disc.tracks.len(), 0);
        assert_eq!(disc.title, "Album");
        Ok(())
    }

    #[test]
    fn test_track_with_special_characters() -> Result<(), GnuDbError> {
        init_logger();
        let data = "DTITLE=Artist / Album\nTTITLE0=Track with / slash\nTTITLE1=Track (with) [brackets] & symbols!\n";
        let disc = parse_read_response(data)?;
        assert_eq!(disc.tracks.len(), 2);
        assert_eq!(disc.tracks[0].title, "Track with / slash");
        assert_eq!(disc.tracks[1].title, "Track (with) [brackets] & symbols!");
        Ok(())
    }

    #[test]
    fn test_year_overflow() -> Result<(), GnuDbError> {
        init_logger();
        // Year too large for u16
        let data = "DTITLE=Artist / Album\nDYEAR=99999\nTTITLE0=Track\n";
        let disc = parse_read_response(data)?;
        // Should fail to parse and fall through without setting year
        assert!(disc.year.is_none());
        Ok(())
    }

    #[test]
    fn test_extd_year_not_used_when_dyear_valid() -> Result<(), GnuDbError> {
        init_logger();
        let data = "DTITLE=Artist / Album\nDYEAR=2020\nTTITLE0=Track\nEXTD= YEAR: 1999\n";
        let disc = parse_read_response(data)?;
        assert_eq!(disc.year, Some(2020));
        Ok(())
    }
}
