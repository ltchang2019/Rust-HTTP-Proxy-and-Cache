use std::{collections::HashMap};
use threadpool::ThreadPool;
use std::net::TcpStream;
use http::{Request, Response, Method};
use std::sync::{Mutex, Arc};
use::dns_lookup;
use super::{cache::HTTPCache, strikeset::HTTPStrikeSet};
use super::http::{request, response};

type ProxyData = (Arc<Mutex<HTTPCache>>, Arc<HTTPStrikeSet>);
type HandlerFn = fn(req: Request<Vec<u8>>, data: ProxyData, conn: &mut TcpStream);

pub struct HTTPRequestHandler {
    pool: ThreadPool,
    handlers: Arc<HashMap<Method, HandlerFn>>,
    strikeset: Arc<HTTPStrikeSet>,
    cache: Arc<Mutex<HTTPCache>>,
}

impl HTTPRequestHandler {
    pub fn new() -> Self {
        let mut map: HashMap<Method, HandlerFn> = HashMap::new();
        map.insert(Method::GET, HTTPRequestHandler::handle_get);
        map.insert(Method::POST, HTTPRequestHandler::handle_post);
        map.insert(Method::HEAD, HTTPRequestHandler::handle_head);

        Self {
            pool: ThreadPool::new(64),
            handlers: Arc::new(map),
            strikeset: Arc::new(HTTPStrikeSet{}),
            cache: Arc::new(Mutex::new(HTTPCache::new("./cache"))),
        }
    }

    pub fn service_req(&self, mut client_conn: TcpStream) {
        let data = (self.cache.clone(), self.strikeset.clone());
        let handlers = self.handlers.clone();

        self.pool.execute(move || {
            match request::read_from_stream(&mut client_conn) {
                Ok(req) => {
                    handlers[req.method()](req, data, &mut client_conn);
                },
                Err(e) => {}
            }
        });
    }

    fn handle_get(mut req: Request<Vec<u8>>, data: ProxyData, client_conn: &mut TcpStream) {
        let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
        request::extend_header_value(&mut req, "x-forwarded-for", &client_ip);

        let hostname = req.uri().host().unwrap();
        let host_ip = dns_lookup::lookup_host(hostname).unwrap();
        let mut host_conn = TcpStream::connect(format!("{}:{}", host_ip[0], "80")).unwrap();

        let res: Response<Vec<u8>>;
        let cache = data.0.lock().unwrap();
        if cache.contains_entry(&req) {
            match cache.get_cached_response(&req) {
                Some(cached_res) => res = cached_res,
                None => res = Self::forward_request_and_return_response(&req, &mut host_conn)
            }
        } else {
            res = Self::forward_request_and_return_response(&req, &mut host_conn);
            cache.add_entry(&req, &res);
        }
        response::send_response(client_conn, &res);
    }

    fn handle_post(mut req: Request<Vec<u8>>, data: ProxyData, client_conn: &mut TcpStream) {
        let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
        request::extend_header_value(&mut req, "x-forwarded-for", &client_ip);

        let hostname = req.uri().host().unwrap();
        let host_ip = dns_lookup::lookup_host(hostname).unwrap();
        let mut host_conn = TcpStream::connect(format!("{}:{}", host_ip[0], "80")).unwrap();

        let res = Self::forward_request_and_return_response(&req, &mut host_conn);
        response::send_response(client_conn, &res);
    }

    fn handle_head(mut req: Request<Vec<u8>>, data: ProxyData, client_conn: &mut TcpStream) {
        let client_ip = client_conn.peer_addr().unwrap().ip().to_string();
        request::extend_header_value(&mut req, "x-forwarded-for", &client_ip);

        let hostname = req.uri().host().unwrap();
        let host_ip = dns_lookup::lookup_host(hostname).unwrap();
        let mut host_conn = TcpStream::connect(format!("{}:{}", host_ip[0], "80")).unwrap();

        let res = Self::forward_request_and_return_response(&req, &mut host_conn);
        response::send_response(client_conn, &res);
    }

    pub fn forward_request_and_return_response(req: &Request<Vec<u8>>, host_conn: &mut TcpStream) -> Response<Vec<u8>> {
        if let Err(err) = request::write_to_stream(&req, host_conn) {
            log::error!("Failed to send request to host {:?}: {:?}", host_conn.peer_addr().unwrap().ip(), err);
            return response::make_http_error(http::StatusCode::BAD_GATEWAY);
        }

        let host_res = match response::read_from_stream(host_conn, req.method()) {
            Ok(res) => res,
            Err(error) => {
                log::error!("Error reading response from server: {:?}", error);
                return response::make_http_error(http::StatusCode::BAD_GATEWAY);
            }
        };
        host_res
    }
}