extern crate chrono;
extern crate timer;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, SyncSender, Sender};
use std::sync::{mpsc};
use std::thread;

use log::{debug, info, warn};
use timer::Guard;
use warp::Filter;

use crate::config::Settings;
use crate::notifier::{NoOpNotifier, Notifier};
use crate::notifiers::webhook::WebhookNotifier;

mod notifier;

mod notifiers { pub mod webhook; }

mod config;

fn build_notifier(cfg: &Settings) -> Result<Box<dyn Notifier>, String> {
    match cfg.notifier_type.as_str() {
        "webhook" => match cfg.webhook {
            Some(ref wh) => Ok(
                Box::new(WebhookNotifier::new(
                    wh.url.clone(),
                    wh.method.clone(),
                    wh.headers.clone().unwrap_or(vec![])
                )),
            ),
            None => Err("no webhook settings found".to_string()),
        },
        "noop" => Ok(Box::new(NoOpNotifier {})),
        t => Err(format!("unsupported notifier: {}", t))
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let settings = config::retrieve_settings(Some("dodemansknop.json")).unwrap();

    info!("loaded settings: {:?}", settings);

    let (tx_ping, rx_ping): (SyncSender<String>, Receiver<String>) = mpsc::sync_channel(32);
    let (tx_alert, rx_alert): (Sender<String>, Receiver<String>) = mpsc::channel();
    let notifier = build_notifier(&settings).unwrap();

    thread::spawn(move || {
        loop {
            let r = rx_alert.recv();
            if r.is_err() {
                warn!("error while receiving alert: {}", r.err().unwrap());
                continue;
            }

            match notifier.notify_failure(&r.unwrap()) {
                Ok(_) => info!("failure notified"),
                Err(e) => warn!("error while notifying about failure: {}", e)
            }
        }
    });

    thread::spawn(move || {
        let timer = timer::Timer::new();
        let delay = chrono::Duration::seconds(5);

        let mut active_timers: HashMap<String, Guard> = HashMap::new();

        loop {
            let r = rx_ping.recv();
            if r.is_err() {
                warn!("error while receiving ping: {}", r.err().unwrap());
                continue;
            }

            let id = r.unwrap();
            let idc = id.clone();

            let tx_cpy = tx_alert.clone();

            debug!("received ping for {}", id);

            active_timers.insert(id, timer.schedule_with_delay(delay, move || {
                info!("missed ping for {}; scheduling alert", idc);

                match tx_cpy.send(idc.clone()) {
                    Ok(_) => debug!("alert scheduled for {}", idc),
                    Err(e) => warn!("error while scheduling alert: {}", e)
                }
            }));
        }
    });

    let api = filters::ping(tx_ping);
    let routes = api.with(warp::log("ping"));

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}

mod filters {
    use std::convert::Infallible;
    use std::sync::mpsc::SyncSender;

    use warp::Filter;

    use super::handlers;

    pub fn ping(ping_tx: SyncSender<String>) -> impl Filter<Extract=impl warp::Reply, Error=warp::Rejection> + Clone {
        warp::path!("ping" / String)
            .and(warp::post())
            .and(with_ping_tx(ping_tx))
            .and_then(handlers::ping)
    }

    fn with_ping_tx(tx: SyncSender<String>) -> impl Filter<Extract=(SyncSender<String>, ), Error=Infallible> + Clone {
        warp::any().map(move || tx.clone())
    }
}

mod handlers {
    use std::convert::Infallible;
    use std::sync::mpsc::SyncSender;

    use log::warn;
    use warp::http::StatusCode;

    pub async fn ping(id: String, tx: SyncSender<String>) -> Result<impl warp::Reply, Infallible> {
        match tx.send(id) {
            Ok(_) => Ok(StatusCode::OK),
            Err(err) => {
                warn!("error while sending ping to handler thread: {}", err);
                Ok(StatusCode::SERVICE_UNAVAILABLE)
            }
        }
    }
}