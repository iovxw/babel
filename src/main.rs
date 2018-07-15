#![feature(uniform_paths)]
#![feature(nll)]
#![feature(proc_macro_non_items)]
#![feature(generators)]
#![feature(transpose_result)]

use ::atom_syndication as atom;
use ::chrono;
use ::env_logger;
use ::failure;
use ::serde_derive::Deserialize;
use ::serde_json;

use std::collections::HashMap;
use std::fs::File;
use std::net::SocketAddr;

use ::actix_web::{
    self, dev::AsyncResult, http, server, App, Either, HttpMessage, HttpResponse, Path, Responder,
};
use ::failure::ResultExt;
use ::futures_await::{self as futures, prelude::{await, async_block, *}};
use ::scraper::{ElementRef, Html};
use ::structopt::StructOpt;
use ::uuid::{self, Uuid};

mod selector;

use selector::{Selector, SelectorEx};

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:58.0) Gecko/20100101 Firefox/58.0";

static mut CONFIG: *const HashMap<String, Feed> = std::ptr::null();

#[derive(StructOpt, Debug)]
struct Opt {
    /// Listening address
    #[structopt(short = "a", long = "addr", default_value = "127.0.0.1:8777")]
    addr: SocketAddr,
    #[structopt(
        short = "c",
        long = "config",
        default_value = "./services.json"
    )]
    config: String,
}

#[derive(Deserialize, Debug)]
struct Feed {
    title: String,
    subtitle: Option<String>,
    link: String,
    entries: Selector,
    entry_title: SelectorEx,
    entry_link: Option<SelectorEx>,
    entry_author: Option<SelectorEx>,
    entry_summary: Option<SelectorEx>,
    entry_updated: Option<SelectorEx>,
    entry_published: Option<SelectorEx>,
}

fn get_config() -> &'static HashMap<String, Feed> {
    unsafe { &*CONFIG }
}

fn init_config(path: &str) -> Result<(), failure::Error> {
    let config: HashMap<String, Feed> = serde_json::from_reader(
        File::open(path).context(format!("Failed to open config file: {}", path))?,
    ).context(format!("Failed to parse config file: {}", path))?;
    unsafe {
        // Put on the heap to make it 'static
        CONFIG = Box::into_raw(Box::new(config));
    }
    Ok(())
}

fn select(entry_element: &ElementRef, selector: &SelectorEx) -> Result<String, actix_web::Error> {
    let element = entry_element
        .select(&selector.selector)
        .next()
        .ok_or_else(|| {
            actix_web::error::ErrorInternalServerError(format!("selector: {:?}", selector))
        })?;
    let r = if let Some(attr) = &selector.attr {
        element
            .value()
            .attr(attr)
            .ok_or_else(|| {
                actix_web::error::ErrorInternalServerError(format!("selector: {:?}", selector))
            })?.to_string()
    } else {
        element.text().collect()
    };
    Ok(r)
}

fn fill_entry(entry_element: ElementRef, feed_cfg: &Feed) -> Result<atom::Entry, actix_web::Error> {
    let mut entry = atom::Entry::default();
    let title = select(&entry_element, &feed_cfg.entry_title)?;
    entry.set_title(title);
    let link = feed_cfg
        .entry_link
        .as_ref()
        .map(|s| select(&entry_element, s))
        .transpose()?
        .map(|mut l| {
            let mut link = atom::Link::default();
            if l.starts_with(&['/', '.'][..]) {
                l = feed_cfg.link.to_owned() + &l;
            } else if !l.starts_with("http") {
                l = feed_cfg.link.to_owned() + "/" + &l;
            }
            link.set_href(l);
            link
        });
    entry.set_links(link.into_iter().collect::<Vec<atom::Link>>());
    let id = Uuid::new_v5(
        &uuid::NAMESPACE_URL,
        &format!("{}{:?}", entry.title(), entry.links()),
    );
    entry.set_id(id.urn().to_string());
    let author = feed_cfg
        .entry_author
        .as_ref()
        .map(|s| select(&entry_element, s))
        .transpose()?
        .map(|l| {
            let mut author = atom::Person::default();
            author.set_name(l);
            author
        });
    entry.set_authors(author.into_iter().collect::<Vec<atom::Person>>());
    let summary = feed_cfg
        .entry_summary
        .as_ref()
        .map(|s| select(&entry_element, s))
        .transpose()?;
    entry.set_summary(summary);
    let updated = feed_cfg
        .entry_updated
        .as_ref()
        .map(|s| select(&entry_element, s))
        .transpose()?;
    entry.set_updated(updated.unwrap_or_else(|| String::new()));
    let published = feed_cfg
        .entry_published
        .as_ref()
        .map(|s| select(&entry_element, s))
        .transpose()?;
    entry.set_published(published);
    Ok(entry)
}

fn index(
    info: Path<String>,
) -> impl Responder<Item = AsyncResult<HttpResponse>, Error = actix_web::Error> {
    if let Some(feed_cfg) = get_config().get(&*info) {
        Either::B(Box::new(async_block! {
            let resp = await!(actix_web::client::get(&feed_cfg.link)
                              .header("User-Agent", UA)
                              .finish()
                              .expect("request builder")
                              .send())?;

            if !resp.status().is_success() {
                // error
            }
            let body = await!(resp.body().limit(524_288))?;
            let html = Html::parse_document(&String::from_utf8_lossy(&body));

            let mut feed = atom::Feed::default();
            feed.set_title(feed_cfg.title.clone());
            feed.set_subtitle(feed_cfg.subtitle.clone());
            feed.set_updated(chrono::Local::now().to_rfc3339());
            feed.set_id(Uuid::new_v5(&uuid::NAMESPACE_URL, &*info).urn().to_string());
            // feed.set_generator();

            let mut link = atom::Link::default();
            link.set_href(feed_cfg.link.clone());
            feed.set_links(vec![link]);

            let mut entries = Vec::new();
            for entry_element in html.select(&feed_cfg.entries) {
                let entry = fill_entry(entry_element, &feed_cfg)?;
                entries.push(entry);
            }
            if entries.is_empty() {
                return Err(actix_web::error::ErrorInternalServerError("entries selector"));
            }
            feed.set_entries(entries);
            Ok(HttpResponse::Ok().content_type("application/xml").body(feed.to_string()))
        })
            as Box<Future<Item = HttpResponse, Error = actix_web::Error>>)
    } else {
        Either::A(HttpResponse::NotFound())
    }
}

fn main() -> Result<(), failure::Error> {
    let opt = Opt::from_args();

    if ::std::env::var("RUST_LOG").is_err() {
        ::std::env::set_var(
            "RUST_LOG",
            concat!("actix_web=info,", env!("CARGO_PKG_NAME"), "=info"),
        );
    }
    env_logger::init();
    init_config(&opt.config)?;

    server::new(|| {
        App::new()
            .middleware(actix_web::middleware::Logger::default())
            .route("/{id}", http::Method::GET, index)
    }).bind(opt.addr)
    .unwrap()
    .run();
    Ok(())
}
