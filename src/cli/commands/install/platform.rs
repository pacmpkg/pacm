pub(super) fn platform_supported(os_list: &[String], cpu_list: &[String]) -> bool {
    let host_os = node_platform();
    let host_cpu = node_arch();

    let os_ok = if os_list.is_empty() {
        true
    } else {
        let mut allowed = None;
        let mut blocked = false;
        for os in os_list {
            if let Some(stripped) = os.strip_prefix('!') {
                if stripped == host_os {
                    blocked = true;
                }
            } else {
                allowed.get_or_insert(false);
                if os == host_os {
                    allowed = Some(true);
                }
            }
        }
        (!blocked) && allowed.unwrap_or(true)
    };

    let cpu_ok = if cpu_list.is_empty() {
        true
    } else {
        let mut allowed = None;
        let mut blocked = false;
        for cpu in cpu_list {
            if let Some(stripped) = cpu.strip_prefix('!') {
                if stripped == host_cpu {
                    blocked = true;
                }
            } else {
                allowed.get_or_insert(false);
                if cpu == host_cpu {
                    allowed = Some(true);
                }
            }
        }
        (!blocked) && allowed.unwrap_or(true)
    };

    os_ok && cpu_ok
}

pub(super) fn node_platform() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "win32"
    }
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "freebsd")]
    {
        "freebsd"
    }
    #[cfg(target_os = "openbsd")]
    {
        "openbsd"
    }
    #[cfg(target_os = "netbsd")]
    {
        "netbsd"
    }
    #[cfg(target_os = "aix")]
    {
        "aix"
    }
    #[cfg(target_os = "solaris")]
    {
        "sunos"
    }
}

pub(super) fn node_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x64"
    }
    #[cfg(target_arch = "x86")]
    {
        "ia32"
    }
    #[cfg(target_arch = "arm")]
    {
        "arm"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
    #[cfg(target_arch = "mips")]
    {
        "mips"
    }
    #[cfg(target_arch = "powerpc")]
    {
        "ppc"
    }
    #[cfg(target_arch = "powerpc64")]
    {
        "ppc64"
    }
    #[cfg(target_arch = "s390x")]
    {
        "s390x"
    }
    #[cfg(target_arch = "riscv64")]
    {
        "riscv64"
    }
}
