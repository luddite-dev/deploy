#[tokio::test]
async fn test_check_host_ports_finds_bound_port() {
  let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let bound_port = listener.local_addr().unwrap().port();

  let sockets = netstat2::get_sockets_info(
    netstat2::AddressFamilyFlags::all(),
    netstat2::ProtocolFlags::TCP,
  )
  .unwrap();
  let bound_ports: std::collections::HashSet<u16> = sockets
    .into_iter()
    .filter_map(|s| match s.protocol_socket_info {
      netstat2::ProtocolSocketInfo::Tcp(tcp) => Some(tcp.local_port),
      _ => None,
    })
    .collect();

  assert!(
    bound_ports.contains(&bound_port),
    "netstat2 should detect the bound port {}",
    bound_port
  );
  drop(listener);
}
