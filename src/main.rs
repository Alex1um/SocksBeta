use std::net::{TcpListener, TcpStream, SocketAddr, IpAddr, Ipv4Addr};
use std::io::{Read, Write, self};
use std::net::ToSocketAddrs;
use std::time::Duration;
use anyhow::{Result, Error};
use request_errors::CommandNotAllowedError;

#[repr(u8)]
enum SOCKSReply {
    Succeeded = 0x00,
    GeneralSOCKSServerFailture = 0x01,
    ConnectionNotAllowedByRuleset = 0x02,
    NetworkUnreachable = 0x03,
    HostUnreachable = 0x04,
    ConnectionRefused = 0x05,
    TTLExpired = 0x06,
    CommandNotSupported = 0x07,
    AddressTypeNotSupported = 0x08,
}


mod request_errors {
    use std::error::Error;
    use std::fmt::Display;

    #[derive(Debug)]
    pub struct CommandNotAllowedError();

    impl Display for CommandNotAllowedError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Method is not allowed")
        }
    }

    impl Error for CommandNotAllowedError {}


    #[derive(Debug)]
    pub struct AddressNotAllowed();
    
    impl Display for AddressNotAllowed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Address type is not allowed")
        }
    }

    impl Error for AddressNotAllowed {}


}


fn reply(client_stream: &mut TcpStream, version: u8, reply: SOCKSReply, target_addr: &SocketAddr) -> Result<()> {
    let mut reply = vec![version, reply as u8, 0x00];
    if let IpAddr::V4(v4) = target_addr.ip() {
        reply.push(0x01);
        reply.extend_from_slice(&v4.octets());
    }
    reply.extend_from_slice(&target_addr.port().to_be_bytes());
    client_stream.write_all(&reply)?;
    client_stream.flush()?;
    Ok(())
}

fn process_method(client_stream: &mut TcpStream) -> Result<u8> /* version */ {
    
    let mut buf = [0; 2];
    client_stream.read_exact(&mut buf)?;
    let version = buf[0];
    let num_methods = buf[1];

    let mut methods_buf = vec![0; num_methods as usize];
    client_stream.read_exact(&mut methods_buf)?;

    let chosen_method: u8 = 0x00; // Выбираем метод без аутентификации
    client_stream.write_all(&[version, chosen_method])?;
    client_stream.flush()?;
    Ok(version)
}

fn process_request(client_stream: &mut TcpStream) -> Result<SocketAddr> {
    use request_errors::*;

    let mut cmd_buf = [0; 4];
    client_stream.read_exact(&mut cmd_buf)?;
    let cmd = cmd_buf[1];
    let addr_type = cmd_buf[3];

    // Обрабатываем только команду "establish a TCP/IP stream connection"
    if cmd != 0x01 {
        return Err(CommandNotAllowedError().into());
    }

    // Читаем адрес назначения
    let target_addr = match addr_type {
        0x01 => {
            // IPv4 адрес
            let mut ip_buf = [0; 4];
            client_stream.read_exact(&mut ip_buf)?;
            let ip = ip_buf.iter()
                .map(|b| b.to_string())
                .collect::<Vec<String>>()
                .join(".");
            let mut port_buf = [0; 2];
            client_stream.read_exact(&mut port_buf)?;
            let port = u16::from_be_bytes(port_buf);
            let addrs = (IpAddr::from(ip_buf), port)
                .to_socket_addrs()
                .unwrap()
                .next()
                .unwrap();
            addrs
        }
        0x03 => {
            // Доменное имя
            let mut len_buf = [0; 1];
            client_stream.read_exact(&mut len_buf)?;
            let len = len_buf[0] as usize;
            let mut domain_buf = vec![0; len];
            client_stream.read_exact(&mut domain_buf)?;
            let domain = String::from_utf8(domain_buf)?;
            let mut port_buf = [0; 2];
            client_stream.read_exact(&mut port_buf)?;
            let port = u16::from_be_bytes(port_buf);
            format!("{}:{}", domain, port)
                .to_socket_addrs()
                .unwrap()
                .next()
                .unwrap()
        }
        _ => return Err(AddressNotAllowed().into()),
    };
    Ok(target_addr)
}




fn serve(target_stream: &mut TcpStream, client_stream: &mut TcpStream) -> Result<()> {
    let mut client_buffer = [0; 4096];
    let mut target_buffer = [0; 4096];

    client_stream.set_nonblocking(true)?;
    target_stream.set_nonblocking(true)?;


    loop {
        let mut client_closed = false;
        let mut target_closed = false;
        
        match client_stream.read(&mut client_buffer) {
            Ok(0) => {
                client_closed = true;
            }
            Ok(n) => {
                target_stream.write_all(&client_buffer[..n])?;
                target_stream.flush()?;
            }
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    client_closed = true;
                }
            }
        }
        match target_stream.read(&mut target_buffer) {
            Ok(0) => {
                target_closed = true;
            }
            Ok(n) => {
                client_stream.write_all(&target_buffer[..n])?;
                client_stream.flush()?;
            }
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    target_closed = true;
                }
            }
        }
        
        if client_closed || target_closed {
            break;
        }
    }
    Ok(())
}


fn serve_epoll(target_stream: &mut TcpStream, client_stream: &mut TcpStream) -> Result<()> {
    use polling::{Event, Poller, Events};

    let mut client_buffer = [0; 4096];
    let mut target_buffer = [0; 4096];

    client_stream.set_nonblocking(true)?;
    target_stream.set_nonblocking(true)?;

    let poller = Poller::new()?;
    unsafe {
        poller.add(client_stream as &TcpStream, Event::readable(1))?;
        poller.add(target_stream as &TcpStream, Event::readable(2))?;
    }

    let mut events = Events::new();
    loop {
        let mut client_closed = false;
        let mut target_closed = false;
        events.clear();
        poller.wait(&mut events, None)?;
        
        for event in events.iter() {
            println!("new event! {:?}", event);
            match event.key {
                1 => match client_stream.read(&mut client_buffer) {
                    Ok(0) => {
                        client_closed = true;
                    }
                    Ok(n) => {
                        target_stream.write_all(&client_buffer[..n])?;
                        target_stream.flush()?;
                    }
                    Err(e) => {
                        client_closed = true;
                    }
                }
                2 => match target_stream.read(&mut target_buffer) {
                    Ok(0) => {
                        target_closed = true;
                    }
                    Ok(n) => {
                        client_stream.write_all(&target_buffer[..n])?;
                        client_stream.flush()?;
                    }
                    Err(e) => {
                        target_closed = true;
                    }
                }
                _ => {}
            }
        }
        poller.modify(client_stream as &TcpStream, Event::readable(1))?;
        poller.modify(target_stream as &TcpStream, Event::readable(2))?;
        if client_closed || target_closed {
            break;
        }
    }
    Ok(())
}


fn handle_client(mut client_stream: TcpStream) {

    if let Ok(version) = process_method(&mut client_stream) {
        println!("version: {}", version);
        if let Ok(target_addr) = process_request(&mut client_stream) {
            if let Ok(mut target_stream) = TcpStream::connect(&target_addr) {
                println!("target stream: {:?}", target_stream);
                if let Ok(_) = reply(&mut client_stream, version, SOCKSReply::Succeeded, &target_addr) {
                    let _ = serve_epoll(&mut target_stream, &mut client_stream);
                    println!("done to {:?}", target_stream);
                }
                
                let _ = target_stream.shutdown(std::net::Shutdown::Both);

            } else {
                println!("connection error");
                let _ = reply(&mut client_stream, version, SOCKSReply::GeneralSOCKSServerFailture, &target_addr);
            }
        } else {
            println!("request error");
        }
    } else {
        println!("method error");
    }

    let _ = client_stream.shutdown(std::net::Shutdown::Both);
}
    

fn main() {
    // Получаем порт из параметров программы
    let port: u16 = std::env::args()
        .nth(1)
        .or_else(|| {
            println!("Port is not passed. Using 9150...");
            Some("9150".to_owned())
        })
        .expect("Valid or default argument")
        .parse()
        .expect("Invalid port number");
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).unwrap();
    for stream in listener.incoming() {
        match stream {
            Ok(client_stream) => {
                println!("new con! {:?}", client_stream);
                handle_client(client_stream);

            }
            Err(_) => {}
        }
    }
}