use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use config::{Config, File, ConfigError}; // 引入 config
use serde::Deserialize; // 引入 serde

// 定义配置结构体，与 config/default.toml 对应
#[derive(Debug, Deserialize)]
struct Settings {
    port_pairs: Vec<[u16; 2]>, // 使用固定大小数组 [u16; 2] 来强制要求每对必须有两个端口
}

// 定义端口常量
const PORT_A: u16 = 8080; // A 端口
const PORT_B: u16 = 8081; // B 端口

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 加载配置
    let settings = load_config()?;

    println!("Loaded configuration: {:?}", settings);

    if settings.port_pairs.is_empty() {
        println!("No port pairs configured. Exiting.");
        return Ok(());
    }

    // 为配置中的每一对端口启动处理任务
    let mut join_handles = vec![];
    for pair in settings.port_pairs {
        let port_a = pair[0];
        let port_b = pair[1];
        println!("Setting up forwarding for pair: {} <-> {}", port_a, port_b);

        // 为每一对端口启动独立的任务
        let handle = tokio::spawn(async move {
            if let Err(e) = handle_port_pair(port_a, port_b).await {
                eprintln!("Error handling port pair {} <-> {}: {}", port_a, port_b, e);
            }
        });
        join_handles.push(handle);
    }

    println!("All port pair handlers initiated. Waiting for tasks to complete (runs indefinitely)...");

    // 等待所有任务完成（理论上服务器会一直运行）
    for handle in join_handles {
       let _ = handle.await; // 等待每个任务，忽略结果（或者可以进行错误处理）
    }

    Ok(())
}

// 新函数：加载配置
fn load_config() -> Result<Settings, ConfigError> {
    let builder = Config::builder()
        // 添加默认配置文件源
        .add_source(File::with_name("config/default").required(true))
        // 可以添加环境变量等其他源
        // .add_source(config::Environment::with_prefix("APP"))
        ;

    // 构建配置
    let config = builder.build()?;

    // 反序列化配置到 Settings 结构体
    config.try_deserialize()
}

// 新函数：处理单个端口对的逻辑
async fn handle_port_pair(port_a: u16, port_b: u16) -> Result<(), Box<dyn Error>> {
     println!("Initializing pair: {} (A) <-> {} (B)", port_a, port_b);
    // 为当前端口对创建 MPSC channel
    let (tx_a, mut rx_a) = mpsc::channel::<TcpStream>(1);
    let (tx_b, mut rx_b) = mpsc::channel::<TcpStream>(1);

    // 绑定并监听端口 A 和 B
    let listener_a = TcpListener::bind(format!("0.0.0.0:{}", port_a)).await
        .map_err(|e| format!("Failed to bind port {}: {}", port_a, e))?;
    let listener_b = TcpListener::bind(format!("0.0.0.0:{}", port_b)).await
         .map_err(|e| format!("Failed to bind port {}: {}", port_b, e))?;

    println!("Pair {}|{}: Listener A ({}) bound to {}", port_a, port_b, port_a, listener_a.local_addr()?);
    println!("Pair {}|{}: Listener B ({}) bound to {}", port_a, port_b, port_b, listener_b.local_addr()?);

    // 为当前端口对存储活动连接的源地址
    let active_connection_a: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
    let active_connection_b: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));

    // 克隆 Arc
    let active_conn_a_clone_listen = active_connection_a.clone();
    let active_conn_b_clone_listen = active_connection_b.clone();
    let active_conn_a_clone_pair = active_connection_a.clone();
    let active_conn_b_clone_pair = active_connection_b.clone();

    // --- 监听任务 A ---
    tokio::spawn(async move {
        loop {
            match listener_a.accept().await {
                Ok((stream, addr)) => {
                    println!("Pair {}|{}: Port A ({}) received connection from: {}", port_a, port_b, port_a, addr);
                    let mut active_conn = active_conn_a_clone_listen.lock().await;
                    if active_conn.is_some() {
                        println!("Pair {}|{}: Port A ({}) already has active connection {:?}. Rejecting {}.", port_a, port_b, port_a, active_conn.unwrap(), addr);
                        drop(stream);
                        continue;
                    }

                    match tx_a.try_send(stream) {
                        Ok(_) => {
                             println!("Pair {}|{}: Port A ({}) connection from {} sent to pairing channel.", port_a, port_b, port_a, addr);
                             *active_conn = Some(addr);
                        }
                        Err(mpsc::error::TrySendError::Full(_stream)) => {
                            println!("Pair {}|{}: Port A ({}) pairing channel full. Rejecting {}.", port_a, port_b, port_a, addr);
                             drop(_stream);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            eprintln!("Pair {}|{}: Port A ({}) pairing channel closed.", port_a, port_b, port_a);
                            break;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Pair {}|{}: Failed accept on Port A ({}): {}", port_a, port_b, port_a, e);
                    // Consider if listener failure should break the loop or just log
                }
            }
        }
         println!("Pair {}|{}: Listener A ({}) task finished.", port_a, port_b, port_a);
    });

    // --- 监听任务 B ---
    tokio::spawn(async move {
         loop {
            match listener_b.accept().await {
                Ok((stream, addr)) => {
                     println!("Pair {}|{}: Port B ({}) received connection from: {}", port_a, port_b, port_b, addr);
                    let mut active_conn = active_conn_b_clone_listen.lock().await;
                    if active_conn.is_some() {
                          println!("Pair {}|{}: Port B ({}) already has active connection {:?}. Rejecting {}.", port_a, port_b, port_b, active_conn.unwrap(), addr);
                         drop(stream);
                         continue;
                    }
                    match tx_b.try_send(stream) {
                        Ok(_) => {
                            println!("Pair {}|{}: Port B ({}) connection from {} sent to pairing channel.", port_a, port_b, port_b, addr);
                            *active_conn = Some(addr);
                        }
                        Err(mpsc::error::TrySendError::Full(_stream)) => {
                             println!("Pair {}|{}: Port B ({}) pairing channel full. Rejecting {}.", port_a, port_b, port_b, addr);
                             drop(_stream);
                        }
                         Err(mpsc::error::TrySendError::Closed(_)) => {
                             eprintln!("Pair {}|{}: Port B ({}) pairing channel closed.", port_a, port_b, port_b);
                            break;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Pair {}|{}: Failed accept on Port B ({}): {}", port_a, port_b, port_b, e);
                     // Consider if listener failure should break the loop or just log
                }
            }
        }
         println!("Pair {}|{}: Listener B ({}) task finished.", port_a, port_b, port_b);
    });

    println!("Pair {}|{}: Pairing task started.", port_a, port_b);

    // --- 配对和转发任务 (主循环为当前端口对) ---
    loop {
        tokio::select! {
            // biased; // 可选：优先检查退出信号，如果需要的话
            stream_a_option = rx_a.recv() => {
                let stream_b_option = rx_b.recv().await; // 等待 B

                if stream_a_option.is_none() || stream_b_option.is_none() {
                    eprintln!("Pair {}|{}: A pairing channel closed. Exiting pair handler.", port_a, port_b);
                    break;
                }

                let stream_a = stream_a_option.unwrap();
                let stream_b = stream_b_option.unwrap();
                spawn_forwarding_task(stream_a, stream_b, active_conn_a_clone_pair.clone(), active_conn_b_clone_pair.clone(), port_a, port_b);

            },
            stream_b_option = rx_b.recv() => {
                 let stream_a_option = rx_a.recv().await; // 等待 A

                 if stream_b_option.is_none() || stream_a_option.is_none() {
                    eprintln!("Pair {}|{}: A pairing channel closed. Exiting pair handler.", port_a, port_b);
                    break;
                 }

                let stream_b = stream_b_option.unwrap();
                let stream_a = stream_a_option.unwrap();
                spawn_forwarding_task(stream_a, stream_b, active_conn_a_clone_pair.clone(), active_conn_b_clone_pair.clone(), port_a, port_b);
            },
            // 可以添加其他分支，例如用于优雅关闭的信号处理
            // _ = tokio::signal::ctrl_c() => {
            //     println!("Ctrl-C received for pair {}|{}. Shutting down this pair...", port_a, port_b);
            //     break;
            // }
        }

        // 重要：在 select! 之后，我们需要确保 rx_a 和 rx_b 在下一次迭代中仍然有效。
        // 如果 recv() 返回 None，表示 channel 关闭，循环会 break。
        // 如果 recv() 成功，stream 会被移交给 spawn_forwarding_task，我们需要继续等待新的 stream。
        // tokio::select! 宏处理了 Future 的 poll，所以下次循环会重新 poll rx_a.recv() 和 rx_b.recv()。
    }
     println!("Pair {}|{}: Pairing task finished.", port_a, port_b);
     Ok(()) // handle_port_pair 结束
}

// 新函数：启动数据转发任务
fn spawn_forwarding_task(
    mut stream_a: TcpStream,
    mut stream_b: TcpStream,
    active_conn_a_release: Arc<Mutex<Option<SocketAddr>>>,
    active_conn_b_release: Arc<Mutex<Option<SocketAddr>>>,
    port_a: u16, // 用于日志记录
    port_b: u16, // 用于日志记录
) {
     let addr_a = stream_a.peer_addr().ok();
     let addr_b = stream_b.peer_addr().ok();

      println!(
            "Pair {}|{}: Pairing successful: {:?} (A) <-> {:?} (B). Starting data forwarding.",
             port_a, port_b, addr_a, addr_b
      );

    tokio::spawn(async move {
            let (mut rd_a, mut wr_a) = io::split(stream_a);
            let (mut rd_b, mut wr_b) = io::split(stream_b);

            let client_to_server = async {
                let bytes_copied = io::copy(&mut rd_a, &mut wr_b).await?;
                wr_b.shutdown().await?;
                Ok::<_, io::Error>(bytes_copied) // 显式指定 Ok 类型
            };

            let server_to_client = async {
                let bytes_copied = io::copy(&mut rd_b, &mut wr_a).await?;
                wr_a.shutdown().await?;
                Ok::<_, io::Error>(bytes_copied) // 显式指定 Ok 类型
            };

            println!("Pair {}|{}: Forwarding started for {:?} <-> {:?}", port_a, port_b, addr_a, addr_b);

            let result = tokio::try_join!(client_to_server, server_to_client);

            match result {
                 Ok((bytes_a_to_b, bytes_b_to_a)) => {
                    println!(
                         "Pair {}|{}: Forwarding finished for {:?} <-> {:?}. A->B: {} bytes, B->A: {} bytes.",
                          port_a, port_b, addr_a, addr_b, bytes_a_to_b, bytes_b_to_a
                    );
                }
                Err(e) => {
                     eprintln!(
                          "Pair {}|{}: Error during forwarding for {:?} <-> {:?}: {}",
                          port_a, port_b, addr_a, addr_b, e
                    );
                }
            }

            // 转发结束后，清除活动连接状态
             let mut conn_a = active_conn_a_release.lock().await;
             println!("Pair {}|{}: Clearing active connection slot for A ({:?})", port_a, port_b, *conn_a);
             *conn_a = None;
             drop(conn_a); // 显式释放锁

            let mut conn_b = active_conn_b_release.lock().await;
            println!("Pair {}|{}: Clearing active connection slot for B ({:?})", port_a, port_b, *conn_b);
             *conn_b = None;
            drop(conn_b); // 显式释放锁
             println!("Pair {}|{}: Active connection slots for {:?} and {:?} cleared.", port_a, port_b, addr_a, addr_b); // 这行可能有点冗余
    });
}

// --- 旧的配对和转发逻辑（已移入 handle_port_pair 和 spawn_forwarding_task） ---
/*
    println!("Pairing task started. Waiting for connections on both ports...");

    // 任务：配对并转发数据
    loop {
        // ... (代码已移动) ...
    }

    Ok(())
*/ 