use log::debug;
use std::time::Duration;

use crate::error::GnuDbError;
use crate::HELLO_STRING;

const HTTP_PATH: &str = "/~cddb/cddb.cgi";

pub(crate) fn http_request(host: &str, port: u16, cmd: &str) -> Result<String, GnuDbError> {
    let url = format!("http://{host}:{port}{HTTP_PATH}");
    debug!("HTTP request URL: {}", url);
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_body(Some(Duration::from_secs(10)))
        .build();
    let agent: ureq::Agent = config.into();
    let mut response = agent
        .get(&url)
        .query("cmd", cmd)
        .query("hello", HELLO_STRING)
        .query("proto", "6")
        .call()?;
    let body = response.body_mut().read_to_string()?;
    debug!("HTTP response body:\n{}", body);
    Ok(body)
}
