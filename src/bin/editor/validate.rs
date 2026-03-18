pub fn validate_address(addr: &str) -> Result<(), String> {
    if addr.is_empty() {
        return Err("Address cannot be empty.".into());
    }

    let Some(colon_pos) = addr.rfind(':') else {
        return Err(format!("'{}': expected host:port format.", addr));
    };

    let host = &addr[..colon_pos];
    let port_str = &addr[colon_pos + 1..];

    if host.is_empty() {
        return Err("Host part cannot be empty.".into());
    }

    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("'{}': port must be a number 1-65535.", port_str))?;

    if port == 0 {
        return Err("Port must be between 1 and 65535.".into());
    }

    Ok(())
}

pub fn validate_protocol(proto: &str) -> Result<(), String> {
    match proto.to_uppercase().as_str() {
        "TCP" | "UDP" => Ok(()),
        _ => Err(format!("'{}': protocol must be TCP or UDP.", proto)),
    }
}
