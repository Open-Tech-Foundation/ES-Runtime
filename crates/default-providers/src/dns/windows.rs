//! The DNS servers Windows is configured with: those of each network adapter
//! that is up, from `GetAdaptersAddresses` (iphlpapi).
//!
//! The one place in this crate that is not `forbid(unsafe_code)` (DECISIONS
//! D188, ARCHITECTURE §7): Windows keeps its resolver configuration behind
//! this call and no file. The declarations are `windows-sys`'s, already in the
//! tree through tokio, so no crate was added for it. Every `unsafe` block reads
//! memory the call wrote into a buffer this function owns, and says why that
//! read is in bounds.

#![allow(unsafe_code)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_FRIENDLY_NAME, GAA_FLAG_SKIP_MULTICAST,
    GAA_FLAG_SKIP_UNICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
};

/// How many times to grow the buffer when adapters appear between the sizing
/// call and the real one.
const ATTEMPTS: usize = 4;

/// The DNS servers of the adapters that are up, in adapter order, without
/// repeats. Empty when the call fails.
pub(crate) fn adapter_dns_servers() -> Vec<IpAddr> {
    let flags = GAA_FLAG_SKIP_UNICAST
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_FRIENDLY_NAME;
    // 15 KB is Microsoft's suggested starting size; u64 elements give the
    // buffer the 8-byte alignment IP_ADAPTER_ADDRESSES_LH needs.
    let mut size: u32 = 15 * 1024;
    for _ in 0..ATTEMPTS {
        let mut buffer: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `first` points to `buffer`, which is `size` bytes or more and
        // 8-byte aligned, and lives until this iteration ends; `size` tells the
        // call how much it may write. The reserved argument must be null.
        let status = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                flags,
                std::ptr::null(),
                first,
                &mut size,
            )
        };
        match status {
            NO_ERROR => return servers(first),
            // Adapters changed between calls; `size` now holds what is needed.
            ERROR_BUFFER_OVERFLOW => continue,
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// Walks the list the call wrote, starting at `first`.
fn servers(first: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<IpAddr> {
    let mut out = Vec::new();
    let mut adapter = first;
    while !adapter.is_null() {
        // SAFETY: `adapter` is `first` or a `Next` link from a previous entry;
        // GetAdaptersAddresses returned NO_ERROR, so every link points into the
        // buffer the caller still owns, at an initialized entry, or is null.
        let entry = unsafe { &*adapter };
        if entry.OperStatus == IfOperStatusUp {
            let mut server = entry.FirstDnsServerAddress;
            while !server.is_null() {
                // SAFETY: as for `adapter`: the DNS server list is part of the
                // same buffer, and its links are null or valid entries.
                let dns = unsafe { &*server };
                if let Some(ip) = address(
                    dns.Address.lpSockaddr.cast_const(),
                    dns.Address.iSockaddrLength,
                ) && !out.contains(&ip)
                {
                    out.push(ip);
                }
                server = dns.Next;
            }
        }
        adapter = entry.Next;
    }
    out
}

/// The IP address in a `SOCKADDR` of `length` bytes, if it is one.
fn address(
    sockaddr: *const windows_sys::Win32::Networking::WinSock::SOCKADDR,
    length: i32,
) -> Option<IpAddr> {
    if sockaddr.is_null() {
        return None;
    }
    let length = usize::try_from(length).ok()?;
    if length < std::mem::size_of::<u16>() {
        return None;
    }
    // SAFETY: `sockaddr` is non-null and points to `length` bytes in the
    // buffer (the call reports the length with the pointer); the family is the
    // first two bytes, read without assuming alignment.
    let family = unsafe { std::ptr::read_unaligned(sockaddr.cast::<u16>()) };
    match family {
        AF_INET if length >= std::mem::size_of::<SOCKADDR_IN>() => {
            // SAFETY: the length covers a whole SOCKADDR_IN; read unaligned.
            let v4 = unsafe { std::ptr::read_unaligned(sockaddr.cast::<SOCKADDR_IN>()) };
            // SAFETY: every bit pattern of the union is a valid u32.
            let raw = unsafe { v4.sin_addr.S_un.S_addr };
            Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(raw))))
        }
        AF_INET6 if length >= std::mem::size_of::<SOCKADDR_IN6>() => {
            // SAFETY: the length covers a whole SOCKADDR_IN6; read unaligned.
            let v6 = unsafe { std::ptr::read_unaligned(sockaddr.cast::<SOCKADDR_IN6>()) };
            // SAFETY: every bit pattern of the union is a valid [u8; 16].
            let octets = unsafe { v6.sin6_addr.u.Byte };
            let ip = Ipv6Addr::from(octets);
            // Windows lists site-local fec0:0:0:ffff::1-3 on adapters that were
            // never given a server, and they answer nothing; a link-local
            // server needs the interface scope a plain address cannot carry.
            let first = ip.segments()[0];
            if ip.segments()[..3] == [0xfec0, 0, 0] || first & 0xffc0 == 0xfe80 {
                None
            } else {
                Some(IpAddr::V6(ip))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_networked_machine_has_dns_servers_and_none_repeat() {
        // Runs only on Windows — in CI — where this is the code that finds the
        // servers a `resolveDns` without `nameServer` asks.
        let servers = adapter_dns_servers();
        assert!(!servers.is_empty(), "no DNS servers found");
        let unique: std::collections::HashSet<_> = servers.iter().collect();
        assert_eq!(unique.len(), servers.len(), "{servers:?}");
        assert!(servers.iter().all(|ip| !ip.is_unspecified()), "{servers:?}");
    }
}
