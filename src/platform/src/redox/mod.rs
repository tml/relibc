//! sys/socket implementation, following http://pubs.opengroup.org/onlinepubs/009696699/basedefs/sys/socket.h.html

use core::fmt::Write;
use core::mem;
use core::ptr;
use core::slice;
use syscall::data::Stat as redox_stat;
use syscall::data::TimeSpec as redox_timespec;
use syscall::flag::*;
use syscall::{self, Result};

use types::*;
use *;

#[repr(C)]
struct SockData {
    port: in_port_t,
    addr: in_addr_t,
    _pad: [c_char; 8],
}

pub fn e(sys: Result<usize>) -> usize {
    match sys {
        Ok(ok) => ok,
        Err(err) => {
            unsafe {
                errno = err.errno as c_int;
            }
            !0
        }
    }
}

macro_rules! bind_or_connect {
    (bind $path:expr) => {
        concat!("/", $path)
    };
    (connect $path:expr) => {
        $path
    };
    ($mode:ident $socket:expr, $address:expr, $address_len:expr) => {{
        if (*$address).sa_family as c_int != AF_INET {
            errno = syscall::EAFNOSUPPORT;
            return -1;
        }
        if ($address_len as usize) < mem::size_of::<sockaddr>() {
            errno = syscall::EINVAL;
            return -1;
        }
        let data: &SockData = mem::transmute(&(*$address).data);
        let addr = &data.addr;
        let port = in_port_t::from_be(data.port); // This is transmuted from bytes in BigEndian order
        let path = format!(bind_or_connect!($mode "{}.{}.{}.{}:{}"), addr[0], addr[1], addr[2], addr[3], port);

        // Duplicate the socket, and then duplicate the copy back to the original fd
        let fd = e(syscall::dup($socket as usize, path.as_bytes()));
        if (fd as c_int) < 0 {
            return -1;
        }
        let result = syscall::dup2(fd, $socket as usize, &[]);
        let _ = syscall::close(fd);
        if (e(result) as c_int) < 0 {
            return -1;
        }
        0
    }}
}

pub unsafe fn accept(socket: c_int, address: *mut sockaddr, address_len: *mut socklen_t) -> c_int {
    let stream = e(syscall::dup(socket as usize, b"listen")) as c_int;
    if stream < 0 {
        return -1;
    }
    if address != ptr::null_mut()
        && address_len != ptr::null_mut()
        && getpeername(stream, address, address_len) < 0
    {
        return -1;
    }
    stream
}

pub unsafe fn bind(socket: c_int, address: *const sockaddr, address_len: socklen_t) -> c_int {
    bind_or_connect!(bind socket, address, address_len)
}

pub fn brk(addr: *mut c_void) -> *mut c_void {
    unsafe { syscall::brk(addr as usize).unwrap_or(0) as *mut c_void }
}

pub fn chdir(path: *const c_char) -> c_int {
    let path = unsafe { c_str(path) };
    e(syscall::chdir(path)) as c_int
}

pub fn chmod(path: *const c_char, mode: mode_t) -> c_int {
    let path = unsafe { c_str(path) };
    match syscall::open(path, O_WRONLY) {
        Err(err) => e(Err(err)) as c_int,
        Ok(fd) => {
            let res = syscall::fchmod(fd as usize, mode);
            let _ = syscall::close(fd);
            e(res) as c_int
        }
    }
}

pub fn chown(path: *const c_char, owner: uid_t, group: gid_t) -> c_int {
    let path = unsafe { c_str(path) };
    match syscall::open(path, O_WRONLY) {
        Err(err) => e(Err(err)) as c_int,
        Ok(fd) => {
            let res = syscall::fchown(fd as usize, owner as u32, group as u32);
            let _ = syscall::close(fd);
            e(res) as c_int
        }
    }
}

pub fn close(fd: c_int) -> c_int {
    e(syscall::close(fd as usize)) as c_int
}

pub unsafe fn connect(socket: c_int, address: *const sockaddr, address_len: socklen_t) -> c_int {
    bind_or_connect!(connect socket, address, address_len)
}

pub fn dup(fd: c_int) -> c_int {
    e(syscall::dup(fd as usize, &[])) as c_int
}

pub fn dup2(fd1: c_int, fd2: c_int) -> c_int {
    e(syscall::dup2(fd1 as usize, fd2 as usize, &[])) as c_int
}

pub fn exit(status: c_int) -> ! {
    let _ = syscall::exit(status as usize);
    loop {}
}

pub unsafe extern "C" fn execve(
    path: *const c_char,
    argv: *const *mut c_char,
    envp: *const *mut c_char,
) -> c_int {
    use alloc::Vec;
    use syscall::flag::*;

    let mut env = envp;
    while !(*env).is_null() {
        let slice = c_str(*env);
        // Should always contain a =, but worth checking
        if let Some(sep) = slice.iter().position(|&c| c == b'=') {
            // If the environment variable has no name, do not attempt
            // to add it to the env.
            if sep > 0 {
                let mut path = b"env:".to_vec();
                path.extend_from_slice(&slice[..sep]);
                match syscall::open(&path, O_WRONLY | O_CREAT) {
                    Ok(fd) => {
                        // If the environment variable has no value, there
                        // is no need to write anything to the env scheme.
                        if sep + 1 < slice.len() {
                            let n = match syscall::write(fd, &slice[sep + 1..]) {
                                Ok(n) => n,
                                err => {
                                    return e(err) as c_int;
                                }
                            };
                        }
                        // Cleanup after adding the variable.
                        match syscall::close(fd) {
                            Ok(_) => (),
                            err => {
                                return e(err) as c_int;
                            }
                        }
                    }
                    err => {
                        return e(err) as c_int;
                    }
                }
            }
        }
        env = env.offset(1);
    }

    let mut len = 0;
    for i in 0.. {
        if (*argv.offset(i)).is_null() {
            len = i;
            break;
        }
    }

    let mut args: Vec<[usize; 2]> = Vec::with_capacity(len as usize);
    let mut arg = argv;
    while !(*arg).is_null() {
        args.push([*arg as usize, c_str(*arg).len()]);
        arg = arg.offset(1);
    }

    e(syscall::execve(c_str(path), &args)) as c_int
}

pub fn fchdir(fd: c_int) -> c_int {
    let path: &mut [u8] = &mut [0; 4096];
    if e(syscall::fpath(fd as usize, path)) == !0 {
        !0
    } else {
        e(syscall::chdir(path)) as c_int
    }
}

pub fn fchmod(fd: c_int, mode: mode_t) -> c_int {
    e(syscall::fchmod(fd as usize, mode)) as c_int
}

pub fn fchown(fd: c_int, owner: uid_t, group: gid_t) -> c_int {
    e(syscall::fchown(fd as usize, owner as u32, group as u32)) as c_int
}

pub fn fcntl(fd: c_int, cmd: c_int, args: c_int) -> c_int {
    e(syscall::fcntl(fd as usize, cmd as usize, args as usize)) as c_int
}

pub fn fork() -> pid_t {
    e(unsafe { syscall::clone(0) }) as pid_t
}

pub fn fstat(fildes: c_int, buf: *mut stat) -> c_int {
    let mut redox_buf: redox_stat = redox_stat::default();
    match e(syscall::fstat(fildes as usize, &mut redox_buf)) {
        0 => {
            unsafe {
                if !buf.is_null() {
                    (*buf).st_dev = redox_buf.st_dev as dev_t;
                    (*buf).st_ino = redox_buf.st_ino as ino_t;
                    (*buf).st_nlink = redox_buf.st_nlink as nlink_t;
                    (*buf).st_mode = redox_buf.st_mode;
                    (*buf).st_uid = redox_buf.st_uid as uid_t;
                    (*buf).st_gid = redox_buf.st_gid as gid_t;
                    // TODO st_rdev
                    (*buf).st_rdev = 0;
                    (*buf).st_size = redox_buf.st_size as off_t;
                    (*buf).st_blksize = redox_buf.st_blksize as blksize_t;
                    (*buf).st_atim = redox_buf.st_atime as time_t;
                    (*buf).st_mtim = redox_buf.st_mtime as time_t;
                    (*buf).st_ctim = redox_buf.st_ctime as time_t;
                }
            }
            0
        }
        _ => -1,
    }
}

pub fn fsync(fd: c_int) -> c_int {
    e(syscall::fsync(fd as usize)) as c_int
}

pub fn ftruncate(fd: c_int, len: off_t) -> c_int {
    e(syscall::ftruncate(fd as usize, len as usize)) as c_int
}

pub fn getcwd(buf: *mut c_char, size: size_t) -> *mut c_char {
    let buf_slice = unsafe { slice::from_raw_parts_mut(buf as *mut u8, size as usize) };
    if e(syscall::getcwd(buf_slice)) == !0 {
        ptr::null_mut()
    } else {
        buf
    }
}

pub fn getegid() -> gid_t {
    e(syscall::getegid()) as gid_t
}

pub fn geteuid() -> uid_t {
    e(syscall::geteuid()) as uid_t
}

pub fn getgid() -> gid_t {
    e(syscall::getgid()) as gid_t
}

unsafe fn inner_get_name(
    local: bool,
    socket: c_int,
    address: *mut sockaddr,
    address_len: *mut socklen_t,
) -> Result<usize> {
    // 32 should probably be large enough.
    // Format: tcp:remote/local
    // and since we only yet support IPv4 (I think)...
    let mut buf = [0; 32];
    let len = syscall::fpath(socket as usize, &mut buf)?;
    let buf = &buf[..len];
    assert!(&buf[..4] == b"tcp:" || &buf[..4] == b"udp:");
    let buf = &buf[4..];

    let mut parts = buf.split(|c| *c == b'/');
    if local {
        // Skip the remote part
        parts.next();
    }
    let part = parts.next().expect("Invalid reply from netstack");

    let data = slice::from_raw_parts_mut(
        &mut (*address).data as *mut _ as *mut u8,
        (*address).data.len(),
    );

    let len = data.len().min(part.len());
    data[..len].copy_from_slice(&part[..len]);

    *address_len = len as socklen_t;
    Ok(0)
}

pub unsafe fn getpeername(
    socket: c_int,
    address: *mut sockaddr,
    address_len: *mut socklen_t,
) -> c_int {
    e(inner_get_name(false, socket, address, address_len)) as c_int
}

pub fn getpgid(pid: pid_t) -> pid_t {
    e(syscall::getpgid(pid as usize)) as pid_t
}

pub fn getpid() -> pid_t {
    e(syscall::getpid()) as pid_t
}

pub fn getppid() -> pid_t {
    e(syscall::getppid()) as pid_t
}

pub unsafe fn getsockname(
    socket: c_int,
    address: *mut sockaddr,
    address_len: *mut socklen_t,
) -> c_int {
    e(inner_get_name(true, socket, address, address_len)) as c_int
}

pub fn getsockopt(
    socket: c_int,
    level: c_int,
    option_name: c_int,
    option_value: *mut c_void,
    option_len: *mut socklen_t,
) -> c_int {
    let _ = write!(
        ::FileWriter(2),
        "unimplemented: getsockopt({}, {}, {}, {:p}, {:p})",
        socket,
        level,
        option_name,
        option_value,
        option_len
    );
    -1
}

pub fn getuid() -> uid_t {
    e(syscall::getuid()) as pid_t
}

pub fn kill(pid: pid_t, sig: c_int) -> c_int {
    e(syscall::kill(pid, sig as usize)) as c_int
}

pub fn killpg(pgrp: pid_t, sig: c_int) -> c_int {
    e(syscall::kill(-(pgrp as isize) as pid_t, sig as usize)) as c_int
}

pub fn link(path1: *const c_char, path2: *const c_char) -> c_int {
    let path1 = unsafe { c_str(path1) };
    let path2 = unsafe { c_str(path2) };
    e(unsafe { syscall::link(path1.as_ptr(), path2.as_ptr()) }) as c_int
}

pub fn listen(socket: c_int, backlog: c_int) -> c_int {
    // TODO
    0
}

pub fn lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t {
    e(syscall::lseek(
        fd as usize,
        offset as isize,
        whence as usize,
    )) as off_t
}

pub fn lstat(path: *const c_char, buf: *mut stat) -> c_int {
    let path = unsafe { c_str(path) };
    match syscall::open(path, O_RDONLY | O_NOFOLLOW) {
        Err(err) => e(Err(err)) as c_int,
        Ok(fd) => {
            let res = fstat(fd as i32, buf);
            let _ = syscall::close(fd);
            res
        }
    }
}

pub fn mkdir(path: *const c_char, mode: mode_t) -> c_int {
    let flags = O_CREAT | O_EXCL | O_CLOEXEC | O_DIRECTORY | mode as usize & 0o777;
    let path = unsafe { c_str(path) };
    match syscall::open(path, flags) {
        Ok(fd) => {
            let _ = syscall::close(fd);
            0
        }
        Err(err) => e(Err(err)) as c_int,
    }
}

pub fn mkfifo(path: *const c_char, mode: mode_t) -> c_int {
    let flags = O_CREAT | MODE_FIFO as usize | mode as usize & 0o777;
    let path = unsafe { c_str(path) };
    match syscall::open(path, flags) {
        Ok(fd) => {
            let _ = syscall::close(fd);
            0
        }
        Err(err) => e(Err(err)) as c_int,
    }
}

pub fn nanosleep(rqtp: *const timespec, rmtp: *mut timespec) -> c_int {
    let redox_rqtp = unsafe { redox_timespec::from(&*rqtp) };
    let mut redox_rmtp: redox_timespec;
    if rmtp.is_null() {
        redox_rmtp = redox_timespec::default();
    } else {
        redox_rmtp = unsafe { redox_timespec::from(&*rmtp) };
    }
    match e(syscall::nanosleep(&redox_rqtp, &mut redox_rmtp)) as c_int {
        -1 => -1,
        _ => {
            unsafe {
                if !rmtp.is_null() {
                    (*rmtp).tv_sec = redox_rmtp.tv_sec;
                    (*rmtp).tv_nsec = redox_rmtp.tv_nsec as i64;
                }
            }
            0
        }
    }
}

pub fn open(path: *const c_char, oflag: c_int, mode: mode_t) -> c_int {
    let path = unsafe { c_str(path) };
    e(syscall::open(path, (oflag as usize) | (mode as usize))) as c_int
}

pub fn pipe(fds: &mut [c_int]) -> c_int {
    let mut usize_fds: [usize; 2] = [0; 2];
    let res = e(syscall::pipe2(&mut usize_fds, 0));
    fds[0] = usize_fds[0] as c_int;
    fds[1] = usize_fds[1] as c_int;
    res as c_int
}

pub fn read(fd: c_int, buf: &mut [u8]) -> ssize_t {
    e(syscall::read(fd as usize, buf)) as ssize_t
}

pub unsafe fn recvfrom(
    socket: c_int,
    buf: *mut c_void,
    len: size_t,
    flags: c_int,
    address: *mut sockaddr,
    address_len: *mut socklen_t,
) -> ssize_t {
    if flags != 0 {
        errno = syscall::EOPNOTSUPP;
        return -1;
    }
    if address != ptr::null_mut()
        && address_len != ptr::null_mut()
        && getpeername(socket, address, address_len) < 0
    {
        return -1;
    }
    read(socket, slice::from_raw_parts_mut(buf as *mut u8, len))
}

pub fn rename(oldpath: *const c_char, newpath: *const c_char) -> c_int {
    let (oldpath, newpath) = unsafe { (c_str(oldpath), c_str(newpath)) };
    match syscall::open(oldpath, O_WRONLY) {
        Ok(fd) => {
            let retval = syscall::frename(fd, newpath);
            let _ = syscall::close(fd);
            e(retval) as c_int
        }
        err => e(err) as c_int,
    }
}

pub fn rmdir(path: *const c_char) -> c_int {
    let path = unsafe { c_str(path) };
    e(syscall::rmdir(path)) as c_int
}

pub unsafe fn sendto(
    socket: c_int,
    buf: *const c_void,
    len: size_t,
    flags: c_int,
    dest_addr: *const sockaddr,
    dest_len: socklen_t,
) -> ssize_t {
    if dest_addr != ptr::null() || dest_len != 0 {
        errno = syscall::EISCONN;
        return -1;
    }
    if flags != 0 {
        errno = syscall::EOPNOTSUPP;
        return -1;
    }
    write(socket, slice::from_raw_parts(buf as *const u8, len))
}

pub fn setpgid(pid: pid_t, pgid: pid_t) -> c_int {
    e(syscall::setpgid(pid as usize, pgid as usize)) as c_int
}

pub fn setregid(rgid: gid_t, egid: gid_t) -> c_int {
    e(syscall::setregid(rgid as usize, egid as usize)) as c_int
}

pub fn setreuid(ruid: uid_t, euid: uid_t) -> c_int {
    e(syscall::setreuid(ruid as usize, euid as usize)) as c_int
}

pub fn setsockopt(
    socket: c_int,
    level: c_int,
    option_name: c_int,
    option_value: *const c_void,
    option_len: socklen_t,
) -> c_int {
    let _ = write!(
        ::FileWriter(2),
        "unimplemented: setsockopt({}, {}, {}, {:p}, {})",
        socket,
        level,
        option_name,
        option_value,
        option_len
    );
    -1
}

pub fn shutdown(socket: c_int, how: c_int) -> c_int {
    let _ = write!(
        ::FileWriter(2),
        "unimplemented: shutdown({}, {})",
        socket,
        how
    );
    -1
}

pub fn stat(path: *const c_char, buf: *mut stat) -> c_int {
    let path = unsafe { c_str(path) };
    match syscall::open(path, O_RDONLY) {
        Err(err) => e(Err(err)) as c_int,
        Ok(fd) => {
            let res = fstat(fd as i32, buf);
            let _ = syscall::close(fd);
            res
        }
    }
}

pub unsafe fn socket(domain: c_int, mut kind: c_int, protocol: c_int) -> c_int {
    if domain != AF_INET {
        errno = syscall::EAFNOSUPPORT;
        return -1;
    }
    if protocol != 0 {
        errno = syscall::EPROTONOSUPPORT;
        return -1;
    }

    let mut flags = O_RDWR;
    if kind & SOCK_NONBLOCK == SOCK_NONBLOCK {
        kind &= !SOCK_NONBLOCK;
        flags |= O_NONBLOCK;
    }
    if kind & SOCK_CLOEXEC == SOCK_CLOEXEC {
        kind &= !SOCK_CLOEXEC;
        flags |= O_CLOEXEC;
    }

    // The tcp: and udp: schemes allow using no path,
    // and later specifying one using `dup`.
    match kind {
        SOCK_STREAM => e(syscall::open("tcp:", flags)) as c_int,
        SOCK_DGRAM => e(syscall::open("udp:", flags)) as c_int,
        _ => {
            errno = syscall::EPROTOTYPE;
            -1
        }
    }
}

pub fn socketpair(domain: c_int, kind: c_int, protocol: c_int, socket_vector: *mut c_int) -> c_int {
    let _ = write!(
        ::FileWriter(2),
        "unimplemented: socketpair({}, {}, {}, {:p})",
        domain,
        kind,
        protocol,
        socket_vector
    );
    -1
}

pub fn unlink(path: *const c_char) -> c_int {
    let path = unsafe { c_str(path) };
    e(syscall::unlink(path)) as c_int
}

pub fn waitpid(pid: pid_t, stat_loc: *mut c_int, options: c_int) -> pid_t {
    unsafe {
        let mut temp: usize = 0;
        let res = e(syscall::waitpid(pid as usize, &mut temp, options as usize));
        if !stat_loc.is_null() {
            *stat_loc = temp as c_int;
        }
        res
    }
}

pub fn write(fd: c_int, buf: &[u8]) -> ssize_t {
    e(syscall::write(fd as usize, buf)) as ssize_t
}

pub fn clock_gettime(clk_id: clockid_t, tp: *mut timespec) -> c_int {
    let mut redox_tp = unsafe { redox_timespec::from(&*tp) };
    match e(syscall::clock_gettime(clk_id as usize, &mut redox_tp)) as c_int {
        -1 => -1,
        _ => {
            unsafe {
                (*tp).tv_sec = redox_tp.tv_sec;
                (*tp).tv_nsec = redox_tp.tv_nsec as i64;
            };
            0
        }
    }
}
