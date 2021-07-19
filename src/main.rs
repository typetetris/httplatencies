use std::cmp::max;
use crate::headers::{cycle_headers, HeaderNamePathPair, HeaderValues};
use histogram::Histogram;
use reqwest::{Client, StatusCode};
use std::io::{stdout, Write};
use std::iter::repeat;
use std::net;
use std::result::Result;
use std::sync::Arc;
use structopt::StructOpt;
use tokio::{sync, time};

mod headers;

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
    ///
    /// Example: -h Authorization:/tmp/auth-tokens.txt
    #[structopt(short, long, parse(try_from_os_str = HeaderNamePathPair::try_from_os_string))]
    headers_from_file: Vec<HeaderNamePathPair>,

    /// Use that many http client object. Defaults to the
    /// number of local IPs.
    #[structopt(short, long)]
    clients: Option<usize>,
}

#[derive(Debug)]
struct StatsData {
    task_number: u32,
    duration: Vec<Result<time::Duration, String>>,
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
        let mut clients = Vec::new();
        let client_count = max(1, opt.clients.unwrap_or(opt.local_ips.len()));
        if opt.local_ips.is_empty() {
            for _ in 0..client_count {
                clients.push(client_builder().build().unwrap());
            }
        } else {
            for (_, local_ip) in (0..client_count).zip(opt.local_ips.into_iter().cycle()) {
                clients.push(
                    client_builder()
                        .local_address(Some(local_ip))
                        .build()
                        .unwrap(),
                );
            }
        }
        clients
    };

    let header_values = opt
        .headers_from_file
        .into_iter()
        .map(HeaderValues::new)
        .collect::<Result<Vec<HeaderValues>, String>>()?;
    type TRPair = (
        sync::mpsc::UnboundedSender<StatsData>,
        sync::mpsc::UnboundedReceiver<StatsData>,
    );
    let (sender, mut receiver): TRPair = sync::mpsc::unbounded_channel();

    let jitter = time::Duration::from_secs(1) / opt.task_count;
    let probe_count = opt.probe_count;
    let barrier = Arc::new(sync::Barrier::new(opt.task_count as usize));
    for (((((task_number, url), client), headers), sender), barrier) in (0..opt.task_count)
        .zip(opt.urls.into_iter().cycle())
        .zip(clients.into_iter().cycle())
        .zip(cycle_headers(&header_values[..]))
        .zip(repeat(sender))
        .zip(repeat(barrier))
    {
        tokio::spawn(async move {
            take_measurments(
                task_number,
                client,
                url,
                probe_count,
                sender,
                headers,
                jitter * task_number,
                barrier,
            )
            .await
        });
    }

    println!();

    let mut histogram = Histogram::new();
    let maxcount = opt.task_count * opt.probe_count;
    while let Some(stats) = receiver.recv().await {
        if histogram.entries() % 100 == 0 {
            print!("\x1B[`\x1B[K{}/{}", histogram.entries(), maxcount);
            stdout().flush().unwrap();
        }
        for stat in stats.duration {
            match stat {
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
    sender: sync::mpsc::UnboundedSender<StatsData>,
    headers: reqwest::header::HeaderMap,
    jitter: time::Duration,
    barrier: Arc<sync::Barrier>,
) {
    barrier.wait().await;
    time::sleep(jitter).await;
    let mut data = Vec::new();
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
                    data.push(Ok(duration));
                } else {
                    data.push(Err(format!(
                        "status code not OK: {} body: {}",
                        result.status(),
                        result.text().await.unwrap()
                    )));
                }
            }
            Err(err) => {
                data.push(Err(format!("{}", err)));
                break;
            }
        }
        if data.len() % 10 == 0 {
            sender
                .send(StatsData {
                    task_number,
                    duration: data,
                })
                .unwrap();
            data = Vec::new();
        }
        time::sleep(time::Duration::from_secs(1)).await;
    }

    if !data.is_empty() {
        sender
            .send(StatsData {
                task_number,
                duration: data,
            })
            .unwrap();
    }
}
