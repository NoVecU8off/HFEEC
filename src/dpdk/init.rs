// src/dpdk/init.rs
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;

use crate::dpdk::config::DpdkConfig;
use crate::dpdk::ffi;
use crate::dpdk::hugepages;
use crate::numa::node::NumaNode;

/// Структура для представления порта DPDK
pub struct DpdkPortInfo {
    pub port_id: u16,
    pub if_name: String,
    pub numa_node: Option<usize>,
}

/// Инициализирует DPDK EAL для конкретного узла NUMA
pub fn init_dpdk_for_node(
    node: &NumaNode,
    dpdk_config: &DpdkConfig,
    additional_args: &[String],
) -> Result<(), String> {
    if !hugepages::check_hugepages_available() && dpdk_config.use_huge_pages {
        return Err("Huge pages not available but required by config".to_string());
    }

    let mut eal_args = vec![
        "hfeec".to_string(), // Имя программы
    ];

    let core_mask = node.generate_core_mask();
    eal_args.push(format!("--lcores={}", core_mask));

    eal_args.push("--master-lcore=0".to_string());

    eal_args.extend(node.generate_eal_args(dpdk_config));

    eal_args.extend_from_slice(additional_args);

    println!(
        "Initializing DPDK for NUMA node {} with arguments:",
        node.node_id
    );
    for arg in &eal_args {
        println!("  {}", arg);
    }

    let c_args: Vec<CString> = eal_args
        .iter()
        .map(|arg| CString::new(arg.as_str()).unwrap())
        .collect();

    let mut c_argv: Vec<*mut c_char> = c_args
        .iter()
        .map(|arg| arg.as_ptr() as *mut c_char)
        .collect();

    let ret = unsafe { ffi::rte_eal_init(c_args.len() as c_int, c_argv.as_mut_ptr()) };
    if ret < 0 {
        return Err(format!("Failed to initialize DPDK EAL: error code {}", ret));
    }

    Ok(())
}

/// Конфигурирует порт DPDK для конкретного узла NUMA
pub fn configure_port_for_node(
    node: &NumaNode,
    port_id: u16,
    dpdk_config: &DpdkConfig,
) -> Result<(), String> {
    let is_valid = unsafe { ffi::rte_eth_dev_is_valid_port(port_id) };
    if is_valid == 0 {
        return Err(format!("Invalid port id: {}", port_id));
    }

    let port_socket_id = unsafe {
        let socket_id = ffi::rte_eth_dev_socket_id(port_id);
        if socket_id >= 0 {
            socket_id
        } else {
            -1
        }
    };

    if port_socket_id >= 0
        && port_socket_id as usize != node.node_id
        && dpdk_config.use_numa_on_socket
    {
        return Err(format!(
            "Port {} is on NUMA node {}, but trying to configure for node {}",
            port_id, port_socket_id, node.node_id
        ));
    }

    println!("Configuring port {} on socket {}", port_id, port_socket_id);

    let mbuf_pool = create_mbuf_pool_for_port(port_id, dpdk_config)?;
    if mbuf_pool.is_null() {
        return Err("Failed to create mbuf pool".to_string());
    }

    let mut eth_conf = default_eth_config();

    // Настраиваем Receive Side Scaling (RSS)
    let enable_rss = dpdk_config.use_rss && dpdk_config.num_rx_queues > 1;
    if enable_rss {
        eth_conf.rxmode.mq_mode = ffi::ETH_MQ_RX_RSS;
        eth_conf.rx_adv_conf.rss_conf.rss_hf = dpdk_config.rss_hf;

        if let Some(ref key) = dpdk_config.rss_key {
            eth_conf.rx_adv_conf.rss_conf.rss_key = key.as_ptr() as *mut u8;
            eth_conf.rx_adv_conf.rss_conf.rss_key_len = key.len() as u8;
        }
    }

    // Настраиваем размер Jumbo фреймов
    if dpdk_config.use_jumbo_frames {
        eth_conf.rxmode.max_rx_pkt_len = dpdk_config.max_rx_pkt_len;
        // Для Jumbo фреймов требуется scatter
        eth_conf.rxmode.offloads |= ffi::DEV_RX_OFFLOAD_SCATTER;
    }

    // Включаем аппаратный подсчет контрольных сумм
    if dpdk_config.use_hw_checksum {
        eth_conf.rxmode.offloads |= ffi::DEV_RX_OFFLOAD_CHECKSUM;
        eth_conf.txmode.offloads |= ffi::DEV_TX_OFFLOAD_IPV4_CKSUM
            | ffi::DEV_TX_OFFLOAD_UDP_CKSUM
            | ffi::DEV_TX_OFFLOAD_TCP_CKSUM;
    }

    // Настройка TSO
    if dpdk_config.use_tso {
        println!(
            "Enabling TCP Segmentation Offload (TSO) with MSS: {}",
            dpdk_config.max_tso_segment_size
        );
        eth_conf.txmode.offloads |= ffi::DEV_TX_OFFLOAD_TCP_TSO | ffi::DEV_TX_OFFLOAD_MULTI_SEGS;
    }

    // Настройка UDP TSO (GSO)
    if dpdk_config.use_udp_tso {
        println!(
            "Enabling UDP TSO (GSO) with segment size: {}",
            dpdk_config.max_tso_segment_size
        );
        eth_conf.txmode.offloads |= ffi::DEV_TX_OFFLOAD_UDP_TSO | ffi::DEV_TX_OFFLOAD_MULTI_SEGS;
    }

    // Настройка LRO
    if dpdk_config.use_lro {
        println!("Enabling Large Receive Offload (LRO)");
        eth_conf.rxmode.offloads |= ffi::DEV_RX_OFFLOAD_TCP_LRO;
    }

    // Настройка GRO
    if dpdk_config.use_gro {
        println!(
            "Enabling Generic Receive Offload (GRO) with max size: {}",
            dpdk_config.max_gro_size
        );
        eth_conf.rxmode.offloads |= ffi::DEV_RX_OFFLOAD_TCP_GRO;
        eth_conf.rxmode.offloads |= ffi::DEV_RX_OFFLOAD_SCATTER;
    }

    let ret = unsafe {
        ffi::rte_eth_dev_configure(
            port_id,
            dpdk_config.num_rx_queues,
            dpdk_config.num_tx_queues,
            &eth_conf as *const ffi::RteEthConf as *const c_void,
        )
    };

    if ret < 0 {
        return Err(format!(
            "Failed to configure port {}: error code {}",
            port_id, ret
        ));
    }

    // Настройка RX и TX очередей
    for q in 0..dpdk_config.num_rx_queues {
        let queue_socket_id = match dpdk_config.use_numa_on_socket {
            true => port_socket_id,
            false => -1,
        };

        let ret = unsafe {
            ffi::rte_eth_rx_queue_setup(
                port_id,
                q,
                dpdk_config.rx_ring_size as u16,
                queue_socket_id,
                ptr::null(),
                mbuf_pool,
            )
        };

        if ret < 0 {
            return Err(format!(
                "Failed to setup RX queue {}: error code {}",
                q, ret
            ));
        }
    }

    for q in 0..dpdk_config.num_tx_queues {
        let queue_socket_id = match dpdk_config.use_numa_on_socket {
            true => port_socket_id,
            false => -1,
        };

        let ret = unsafe {
            ffi::rte_eth_tx_queue_setup(
                port_id,
                q,
                dpdk_config.tx_ring_size as u16,
                queue_socket_id,
                ptr::null(),
            )
        };

        if ret < 0 {
            return Err(format!(
                "Failed to setup TX queue {}: error code {}",
                q, ret
            ));
        }
    }

    let ret = unsafe { ffi::rte_eth_dev_start(port_id) };
    if ret < 0 {
        return Err(format!(
            "Failed to start port {}: error code {}",
            port_id, ret
        ));
    }

    if dpdk_config.promiscuous {
        let ret = unsafe { ffi::rte_eth_promiscuous_enable(port_id) };
        if ret < 0 {
            return Err(format!(
                "Failed to enable promiscuous mode: error code {}",
                ret
            ));
        }
    }

    Ok(())
}

/// Создает memory pool для порта в соответствующей NUMA-узлу памяти
fn create_mbuf_pool_for_port(
    port_id: u16,
    dpdk_config: &DpdkConfig,
) -> Result<*mut ffi::RteMempool, String> {
    let port_numa_node = unsafe {
        let node = ffi::rte_eth_dev_socket_id(port_id);
        if node >= 0 {
            Some(node as usize)
        } else {
            None
        }
    };

    println!(
        "Creating memory pool for port {} on NUMA node {:?}",
        port_id, port_numa_node
    );

    let pool_name = match port_numa_node {
        Some(node) => CString::new(format!("mbuf_pool_node{}", node)).unwrap(),
        None => CString::new("mbuf_pool_default").unwrap(),
    };

    let socket_id = port_numa_node.map_or(-1, |id| id as c_int);

    let mbuf_pool = unsafe {
        ffi::rte_pktmbuf_pool_create(
            pool_name.as_ptr(),
            dpdk_config.num_mbufs,
            dpdk_config.mbuf_cache_size,
            0,
            dpdk_config.data_room_size,
            socket_id,
        )
    };

    if mbuf_pool.is_null() {
        Err("Failed to create mbuf pool".to_string())
    } else {
        Ok(mbuf_pool)
    }
}

/// Создает Ethernet конфигурацию по умолчанию
fn default_eth_config() -> ffi::RteEthConf {
    ffi::RteEthConf {
        rxmode: ffi::RteEthRxMode {
            mq_mode: 0,
            max_rx_pkt_len: 0,
            split_hdr_size: 0,
            offloads: 0,
        },
        txmode: ffi::RteEthTxMode {
            mq_mode: 0,
            pvid: 0,
            offloads: 0,
        },
        lpbk_mode: 0,
        rx_adv_conf: ffi::RteEthRxAdvConf {
            rss_conf: ffi::RteEthRssConf {
                rss_key: ptr::null_mut(),
                rss_key_len: 0,
                rss_hf: 0,
            },
        },
        tx_adv_conf: ffi::RteEthTxAdvConf {},
        dcb_capability_en: 0,
        fdir_conf: ffi::RteEthFdirConf {},
        intr_conf: ffi::RteEthIntrConf {},
    }
}

/// Перечисляет доступные порты DPDK и возвращает информацию о них
pub fn enumerate_dpdk_ports() -> Vec<DpdkPortInfo> {
    let mut ports = Vec::new();
    let max_ports = 32;

    for port_id in 0..max_ports {
        let is_valid = unsafe { ffi::rte_eth_dev_is_valid_port(port_id as u16) };
        if is_valid != 0 {
            let numa_node = unsafe {
                let socket_id = ffi::rte_eth_dev_socket_id(port_id as u16);
                if socket_id >= 0 {
                    Some(socket_id as usize)
                } else {
                    None
                }
            };

            // В реальном коде нужно получить имя интерфейса
            let if_name = format!("eth{}", port_id);

            ports.push(DpdkPortInfo {
                port_id: port_id as u16,
                if_name,
                numa_node,
            });
        }
    }

    ports
}

/// Завершает работу DPDK и освобождает ресурсы
pub fn cleanup_dpdk() {
    unsafe {
        ffi::rte_eal_cleanup();
    }
}
