use histogram::Histogram;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, StatusCode};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{BufRead, BufReader, Write, stdout};
use std::net;
use std::os::unix::prelude::OsStringExt;
use std::path::PathBuf;
use std::result::Result;
use structopt::StructOpt;
use tokio::{sync, time};

#[derive(Debug)]
struct HeaderNamePathPair {
    name: HeaderName,
    path: PathBuf,
}

impl HeaderNamePathPair {
    fn try_from_os_string(s: &OsStr) -> Result<HeaderNamePathPair, OsString> {
        let s_as_vec = s.to_os_string().into_vec();
        let mut components = s_as_vec.splitn(2, |c| *c == 0x3a); // 0x3a == ':'
        let (h, v) = components
            .next()
            .and_then(|h| components.next().map(|v| (h, v)))
            .ok_or(
                "no : found, need form header_name:path_to_file_containing_header_value_per_line",
            )?;
        let name = HeaderName::from_bytes(h).map_err(|v| format!("header name invalid: {}", v))?;
        let path = PathBuf::from(OsString::from_vec(v.to_vec()));
        Ok(HeaderNamePathPair { name, path })
    }
}

#[derive(Debug)]
struct HeaderValues {
    name: HeaderName,
    values: Vec<HeaderValue>,
}

struct HeaderValuesCycle<'a> {
    cycles: Vec<(
        HeaderName,
        std::iter::Cycle<std::slice::Iter<'a, HeaderValue>>,
    )>,
}

impl Iterator for HeaderValuesCycle<'_> {
    type Item = HeaderMap;
    fn next(&mut self) -> Option<HeaderMap> {
        let mut result = HeaderMap::new();
        for &mut (ref mut name, ref mut cycle) in self.cycles.iter_mut() {
            match cycle.next() {
                None => return None,
                Some(val) => {
                    result.append(name.clone(), val.clone());
                }
            }
        }
        Some(result)
    }
}

impl HeaderValues {
    fn new(HeaderNamePathPair { name, path }: HeaderNamePathPair) -> Result<HeaderValues, String> {
        let mut values: Vec<HeaderValue> = Vec::new();
        let file = File::open(path).map_err(|v| v.to_string())?;
        let mut buf_reader = BufReader::new(file);
        let mut byte_line: Vec<u8> = Vec::new();
        while 0
            != buf_reader
                .read_until(0x0a, &mut byte_line)
                .map_err(|v| v.to_string())?
        {
            if let Some((last, rest)) = byte_line.split_last() {
                let header_value_bytes = if *last == 0x0a { rest } else { &byte_line[..] };
                values
                    .push(HeaderValue::from_bytes(header_value_bytes).map_err(|v| {
                        format!("{}: '{}'", v, String::from_utf8_lossy(&byte_line))
                    })?);
                byte_line.clear();
            }
        }
        Ok(HeaderValues { name, values })
    }
}

fn cycle_headers(header_values: &[HeaderValues]) -> HeaderValuesCycle {
    HeaderValuesCycle {
        cycles: header_values
            .iter()
            .map(|HeaderValues { name, values }| (name.clone(), values.iter().cycle()))
            .collect(),
    }
}

fn first_error_or_values(
    input: Vec<Result<HeaderValues, String>>,
) -> Result<Vec<HeaderValues>, String> {
    let mut result = Vec::new();
    for elem in input {
        match elem {
            Ok(val) => result.push(val),
            Err(err) => return Err(err),
        }
    }
    Ok(result)
}

#[derive(StructOpt, Debug)]
#[structopt()]
/// bombard endpoints with many get requests
///
/// You request a number of tasks, each task will start
/// doing get requests to one of the given urls.
///
/// The urls are distributed on the tasks in round
/// robin fashion. So if you have fewer tasks then
/// urls, some urls will not be used.
///
/// Having more than one url is mainly used to bombard
/// the same server on different IPs, so we don't
/// run out of ports.
///
/// Also you can specify multiple local IPs for the
/// http clients to use, each task will use one http
/// client and will get it assigned in a round robin
/// fashion on creation. Again this is meant to
/// be used, if you run out of ports with one IP.
struct Opt {
    /// the Urls to bombard with get requests
    #[structopt(short, long, required(true))]
    urls: Vec<reqwest::Url>,

    /// local IPs to use for the http clients
    #[structopt(short, long)]
    local_ips: Vec<net::IpAddr>,

    /// spawn this many concurrent (maybe parallel)
    /// tasks doing the get requests
    #[structopt(short, long, default_value = "100")]
    task_count: u32,

    /// each task will do this many get requests to
    /// its url before it dies
    #[structopt(short, long, default_value = "10")]
    probe_count: u32,

    /// You can specify headers here to be added
    /// to the get requests. For each header you
    /// specify a file, which is expected to contain
    /// header values separated by newlines.
    ///
    /// For example "Authorization" header with
    /// lines of the form "Bearer somenthing"
    /// in the given file.
    ///
    /// The files will be complete read, so
    /// shorten them if you run in memory problems.
    #[structopt(short, long, parse(try_from_os_str = HeaderNamePathPair::try_from_os_string))]
    headers_from_file: Vec<HeaderNamePathPair>,
}

#[derive(Debug)]
struct StatsData {
    task_number: u32,
    duration: Result<time::Duration, String>,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let opt = Opt::from_args();
    let client_builder = || {
        reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .timeout(time::Duration::from_secs(30))
    };
    let clients = {
        let mut clients: Vec<reqwest::Client> = opt
            .local_ips
            .iter()
            .map(|ip| client_builder().local_address(Some(*ip)).build().unwrap())
            .collect();
        if clients.is_empty() {
            clients.push(client_builder().build().unwrap());
        }
        clients
    };
    let header_values = first_error_or_values(
        opt.headers_from_file
            .into_iter()
            .map(HeaderValues::new)
            .collect(),
    )?;
    type TRPair = (
        sync::mpsc::UnboundedSender<Result<StatsData, String>>,
        sync::mpsc::UnboundedReceiver<Result<StatsData, String>>,
    );
    let (sender, mut receiver): TRPair = sync::mpsc::unbounded_channel();

    let jitter = time::Duration::from_secs(1) / opt.task_count;
    println!(
        "jitter per conference sec: {} millisec: {}",
        jitter.as_secs(),
        jitter.subsec_millis()
    );
    let probe_count = opt.probe_count;
    for (((task_number, url), client), headers) in (0..opt.task_count)
        .zip(opt.urls.iter().cycle())
        .zip(clients.iter().cycle())
        .zip(cycle_headers(&header_values[..]))
    {
        let url = url.clone();
        let client = client.clone();
        let headers = headers.clone();
        let sender = sender.clone();

        tokio::spawn(async move {
            take_measurments(task_number, client, url, probe_count, sender, headers).await
        });

        time::sleep(jitter).await;
    }

    drop(sender);

    println!();

    let mut histogram = Histogram::new();
    let maxcount = opt.task_count * opt.probe_count;
    while let Some(stats) = receiver.recv().await {
        if histogram.entries() % 100 == 0 {
            print!("\x1B[`\x1B[K{}/{}", histogram.entries(), maxcount);
            stdout().flush().unwrap();
        }
        match stats {
            Ok(stats) => {
                match stats.duration {
                    Ok(dur) => {
                        let value = dur.as_secs() * 1000 + (dur.subsec_millis() as u64);
                        histogram.increment(value).unwrap();
                    }
                    Err(err) => {
                        println!("{} ERROR {}", stats.task_number, err);
                        println!();
                    }
                }
            }
            Err(err) => {
                println!("ERROR: {}", err);
                println!();
            }
        }
    }
    println!();

    println!(
        "min: {}, max: {}, mean: {}, std. deviation: {}, quartiles: {} {} {} {}",
        histogram.minimum().unwrap(),
        histogram.maximum().unwrap(),
        histogram.mean().unwrap(),
        histogram.stddev().unwrap(),
        histogram.percentile(25.0).unwrap(),
        histogram.percentile(50.0).unwrap(),
        histogram.percentile(75.0).unwrap(),
        histogram.percentile(95.0).unwrap()
    );
    Ok(())
}

async fn take_measurments(
    task_number: u32,
    client: Client,
    url: reqwest::Url,
    probe_count: u32,
    sender: sync::mpsc::UnboundedSender<Result<StatsData, String>>,
    headers: reqwest::header::HeaderMap,
) {
    for _ in 0..probe_count {
        let now = time::Instant::now();
        let res = client
            .get(url.clone())
            .headers(headers.clone())
            .send()
            .await;
        match res {
            Ok(result) => {
                if result.status() == StatusCode::OK {
                    let duration = now.elapsed();
                    sender
                        .send(Ok(StatsData {
                            task_number,
                            duration: Ok(duration),
                        }))
                        .unwrap();
                } else {
                    sender
                        .send(Err(format!(
                            "status code not OK: {} body: {}",
                            result.status(),
                            result.text().await.unwrap()
                        )))
                        .unwrap();
                }
            }
            Err(err) => {
                sender.send(Err(format!("{}", err))).unwrap();
                break;
            }
        }
        time::sleep(time::Duration::from_secs(1)).await;
    }
}
