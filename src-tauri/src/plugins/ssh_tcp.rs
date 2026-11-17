//! TCP connection policy, kept separate from SSH authentication and host trust.
use anyhow::{bail, Context, Result};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

pub(super) fn connect<T>(
    addresses: &[SocketAddr],
    macos: bool,
    mut dial: impl FnMut(SocketAddr, Duration) -> io::Result<T>,
    mut elapsed: impl FnMut() -> Duration,
    mut wait: impl FnMut(Duration),
) -> Result<T> {
    // Darwin NECP returns EHOSTUNREACH (65) while the responsible app's
    // local-network identity/permission is being resolved. BSD sockets do not
    // wait for that resolution. Apple TN3179 recommends retries in this case.
    const POLICY_WAIT: Duration = Duration::from_secs(15);
    let mut last = None;
    let mut pending = addresses.to_vec();
    let mut delay = Duration::from_millis(250);
    let mut retried = false;
    let mut policy_blocked = false;
    loop {
        let mut again = vec![];
        for address in pending {
            let remaining = POLICY_WAIT.saturating_sub(elapsed());
            if retried && remaining.is_zero() {
                break;
            }
            let timeout = if retried {
                Duration::from_secs(5).min(remaining)
            } else {
                Duration::from_secs(5)
            };
            match dial(address, timeout) {
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    let recoverable =
                        macos && local_address(address.ip()) && error.raw_os_error() == Some(65);
                    if recoverable {
                        policy_blocked = true;
                        again.push(address);
                    }
                    last = Some((address, error));
                }
            }
        }
        let remaining = POLICY_WAIT.saturating_sub(elapsed());
        if again.is_empty() || remaining.is_zero() {
            break;
        }
        wait(delay.min(remaining));
        delay = (delay * 2).min(Duration::from_secs(1));
        pending = again;
        retried = true;
    }
    match last {
        Some((address, error)) => Err(error).with_context(|| {
            let hint = if policy_blocked {
                "；请检查 macOS 系统设置 → 隐私与安全性 → 本地网络，以及局域网连接"
            } else {
                ""
            };
            format!("无法建立 SSH TCP 连接（{address}）{hint}")
        }),
        None => bail!("主机未解析到可连接的地址"),
    }
}

fn local_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|ip| ip.is_private() || ip.is_link_local())
            .unwrap_or_else(|| ip.is_unicast_link_local() || ip.is_unique_local()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn mac_local_policy_failure_then_allowed_connects_in_one_request() {
        let attempts = Cell::new(0);
        let clock = Cell::new(Duration::ZERO);
        let stream = connect(
            &["192.168.31.153:22".parse().unwrap()],
            true,
            |_, _| {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    Err(io::Error::from_raw_os_error(65))
                } else {
                    Ok("connected")
                }
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        );
        assert_eq!(stream.unwrap(), "connected");
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn delayed_permission_resolution_recovers_without_another_user_submit() {
        let clock = Cell::new(Duration::ZERO);
        let result = connect(
            &["192.168.31.153:22".parse().unwrap()],
            true,
            |_, _| {
                if clock.get() < Duration::from_secs(6) {
                    Err(io::Error::from_raw_os_error(65))
                } else {
                    Ok(())
                }
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        );
        result.unwrap();
        assert!(clock.get() >= Duration::from_secs(6) && clock.get() < Duration::from_secs(7));
    }

    #[test]
    fn policy_lookup_can_take_several_seconds_but_stops_with_a_deadline() {
        let clock = Cell::new(Duration::ZERO);
        let attempts = Cell::new(0);
        let result: Result<()> = connect(
            &["192.168.31.153:22".parse().unwrap()],
            true,
            |_, _| {
                attempts.set(attempts.get() + 1);
                Err(io::Error::from_raw_os_error(65))
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        );
        assert!(result.is_err());
        assert_eq!(clock.get(), Duration::from_secs(15));
        assert!(attempts.get() > 2 && attempts.get() <= 20);
        assert!(format!("{:#}", result.unwrap_err()).contains("本地网络"));
    }

    #[test]
    fn denied_first_address_does_not_delay_a_working_alternative() {
        let clock = Cell::new(Duration::ZERO);
        let addresses = [
            "[fe80::1%1]:22".parse().unwrap(),
            "192.168.31.153:22".parse().unwrap(),
        ];
        let result = connect(
            &addresses,
            true,
            |address, _| {
                if address.is_ipv6() {
                    Err(io::Error::from_raw_os_error(65))
                } else {
                    Ok(())
                }
            },
            || clock.get(),
            |delay| clock.set(clock.get() + delay),
        );
        result.unwrap();
        assert_eq!(clock.get(), Duration::ZERO);
    }

    #[test]
    fn refusals_public_addresses_and_other_platforms_are_not_retried() {
        for (address, macos, code) in [
            ("192.168.31.153:22", true, 61),
            ("8.8.8.8:22", true, 65),
            ("192.168.31.153:22", false, 65),
            ("127.0.0.1:22", true, 65),
        ] {
            let count = Cell::new(0);
            let result: Result<()> = connect(
                &[address.parse().unwrap()],
                macos,
                |_, _| {
                    count.set(count.get() + 1);
                    Err(io::Error::from_raw_os_error(code))
                },
                || Duration::ZERO,
                |_| panic!("must not wait"),
            );
            let error = result.unwrap_err();
            assert_eq!(count.get(), 1);
            assert_eq!(
                error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
                Some(code)
            );
        }
    }
}
