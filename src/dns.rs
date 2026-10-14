use async_trait::async_trait;
use log::*;
use std::collections::HashSet;
use std::io::Error;
use tokio::net::lookup_host;

#[async_trait]
#[allow(unused_variables)]
pub trait DNSResolve<'a> {
    fn remote(&self) -> &String;
    fn dns(&self) -> &Vec<String>;
    fn reset_dns(&mut self, d: &Vec<String>) -> usize;

    #[allow(dead_code)]
    fn domain(&self) -> String {
        let r = self.remote();
        // Bracketed IPv6 literal: [::1]:port  ->  ::1
        if let Some(end) = r.rfind(']') {
            return r[..end].trim_start_matches('[').to_string();
        }
        // host:port (single ':') -> host. A bare IPv6 literal has
        // multiple ':' and no port, so return it whole.
        match r.rsplit_once(':') {
            Some((host, _port)) if !host.contains(':') => host.to_string(),
            _ => r.to_string(),
        }
    }

    #[allow(dead_code)]
    fn port(&self) -> Option<u16> {
        let r = self.remote();
        let port = if let Some(end) = r.rfind(']') {
            // [..]:port -> take the part after ']'
            r[end + 1..].strip_prefix(':')?
        } else {
            let (host, port) = r.rsplit_once(':')?;
            if host.contains(':') {
                // bare IPv6 literal, no port
                return None;
            }
            port
        };
        port.parse::<u16>().ok()
    }
    fn on_dns_changed(&self, new_addr: &String) {}

    async fn resolve(&mut self) -> Result<usize, Error> {
        let remote = self.remote().clone();
        let res: usize;
        let socka = lookup_host(remote.clone()).await;
        match socka {
            Ok(sk) => {
                let mut last = HashSet::new();
                last.extend(self.dns().clone());
                let mut dns = vec![];
                for addr in sk {
                    let d = addr.to_string();
                    if !last.contains(&d) {
                        info!("Resolve {} -> {}", remote, d);
                        self.on_dns_changed(&d);
                    }
                    dns.push(d);
                }
                res = self.reset_dns(&dns);
            }
            Err(e) => {
                warn!("Failed to resolve {}: {:?}", remote, e);
                if !self.dns().is_empty() {
                    // Keep the last known good address(es) across a
                    // transient resolution failure.
                    res = self.dns().len();
                } else {
                    // No previous value: keep the literal as-is so a
                    // valid IP/IPv6 literal still works even if the
                    // system resolver is unavailable. (A real hostname
                    // simply cannot be used until resolution succeeds.)
                    let d = vec![remote.clone()];
                    res = self.reset_dns(&d);
                }
            }
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy {
        remote: String,
        dns: Vec<String>,
    }

    impl<'a> DNSResolve<'a> for Dummy {
        fn remote(&self) -> &String {
            &self.remote
        }
        fn dns(&self) -> &Vec<String> {
            &self.dns
        }
        fn reset_dns(&mut self, d: &Vec<String>) -> usize {
            self.dns = d.clone();
            self.dns.len()
        }
    }

    fn d(remote: &str) -> Dummy {
        Dummy {
            remote: remote.to_string(),
            dns: vec![],
        }
    }

    #[test]
    fn domain_ipv4_host_port() {
        assert_eq!(d("1.2.3.4:50000").domain(), "1.2.3.4");
        assert_eq!(d("example.com:80").domain(), "example.com");
    }

    #[test]
    fn domain_bracketed_ipv6_strips_brackets_and_port() {
        // Regression: the old split(':') returned "[fd00" here, which then
        // panicked on parse::<SocketAddr>() in the UDP path.
        assert_eq!(d("[fd00:99:0::3]:50000").domain(), "fd00:99:0::3");
        assert_eq!(d("[::1]:443").domain(), "::1");
    }

    #[test]
    fn domain_bare_ipv6_returned_whole() {
        assert_eq!(d("fd00:99:0::3").domain(), "fd00:99:0::3");
    }

    #[test]
    fn port_parsing_ipv4_and_ipv6() {
        assert_eq!(d("1.2.3.4:50000").port(), Some(50000));
        assert_eq!(d("[fd00:99:0::3]:50000").port(), Some(50000));
        assert_eq!(d("[::1]:443").port(), Some(443));
    }

    #[test]
    fn port_absent_for_bare_ipv6_or_hostonly() {
        assert_eq!(d("fd00:99:0::3").port(), None);
        assert_eq!(d("example.com").port(), None);
    }
}
