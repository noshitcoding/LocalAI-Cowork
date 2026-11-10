use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) fn is_public_destination(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    !(first == 0
        || first == 10
        || first == 127
        || (first == 100 && (64..=127).contains(&second))
        || (first == 169 && second == 254)
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 0 && third == 0)
        || (first == 192 && second == 0 && third == 2)
        || (first == 192 && second == 88 && third == 99)
        || (first == 192 && second == 168)
        || (first == 198 && (second == 18 || second == 19))
        || (first == 198 && second == 51 && third == 100)
        || (first == 203 && second == 0 && third == 113)
        || first >= 224)
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = address.segments();
    (segments[0] & 0xe000) == 0x2000
        && !(segments[0] == 0x2001
            && (segments[1] == 0
                || segments[1] == 2
                || segments[1] == 0x0db8
                || (segments[1] & 0xfff0) == 0x0010
                || (segments[1] & 0xfff0) == 0x0020))
        && segments[0] != 0x2002
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_public_destinations() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "198.51.100.8",
            "203.0.113.8",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(
                !is_public_destination(address.parse().unwrap()),
                "accepted {address}"
            );
        }
    }

    #[test]
    fn accepts_public_destinations() {
        for address in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(
                is_public_destination(address.parse().unwrap()),
                "rejected {address}"
            );
        }
    }
}
