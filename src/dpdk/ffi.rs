// ffi.rs
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int, c_uint, c_ushort};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use core_affinity;

use super::packet::PacketData;
use super::pool::PacketDataPool;

#[repr(C)]
pub struct RteMbuf {
    _private: [u8; 0],
}

#[repr(C)]
pub struct RteMempool {
    _private: [u8; 0],
}

#[repr(C)]
pub struct RteEthRssConf {
    pub rss_key: *mut u8,
    pub rss_key_len: u8,
    pub rss_hf: u64,
}

pub const ETH_RSS_IP: u64 = 0x1;
pub const ETH_RSS_TCP: u64 = 0x2;
pub const ETH_RSS_UDP: u64 = 0x4;
pub const ETH_RSS_SCTP: u64 = 0x8;
pub const ETH_MQ_RX_RSS: u32 = 1;
pub const ETH_RSS_NONFRAG_IPV4_TCP: u64 = 0x40;
pub const ETH_RSS_NONFRAG_IPV4_UDP: u64 = 0x80;
pub const ETH_RSS_L4_DST_ONLY: u64 = 0x200;
pub const ETH_RSS_L4_SRC_ONLY: u64 = 0x100;

#[repr(C)]
pub struct DpdkConfig {
    pub port_id: c_ushort,
    pub num_rx_queues: c_ushort,
    pub num_tx_queues: c_ushort,
    pub promiscuous: bool,
    pub rx_ring_size: c_uint,
    pub tx_ring_size: c_uint,
    pub num_mbufs: c_uint,
    pub mbuf_cache_size: c_uint,
    pub burst_size: c_uint,
    pub enable_rss: bool,
    pub rss_hf: u64,
    pub use_cpu_affinity: bool,
    pub rss_key: Option<Vec<u8>>,
    pub use_huge_pages: bool,
    pub socket_mem: Option<Vec<u32>>,
    pub huge_dir: Option<String>,
    pub data_room_size: c_ushort,
    pub use_numa_on_socket: bool,
}

#[repr(C)]
pub struct RteEthConf {
    pub rxmode: RteEthRxMode,
    pub txmode: RteEthTxMode,
    pub lpbk_mode: u32,
    pub rx_adv_conf: RteEthRxAdvConf,
    pub tx_adv_conf: RteEthTxAdvConf,
    pub dcb_capability_en: u32,
    pub fdir_conf: RteEthFdirConf,
    pub intr_conf: RteEthIntrConf,
}

#[repr(C)]
pub struct RteEthRxMode {
    pub mq_mode: u32,
    pub max_rx_pkt_len: u32,
    pub split_hdr_size: u16,
    pub offloads: u64,
}

#[repr(C)]
pub struct RteEthTxMode {
    pub mq_mode: u32,
    pub pvid: u16,
    pub offloads: u64,
}

#[repr(C)]
pub struct RteEthRxAdvConf {
    pub rss_conf: RteEthRssConf,
}

#[repr(C)]
pub struct RteEthTxAdvConf {}

#[repr(C)]
pub struct RteEthFdirConf {}

#[repr(C)]
pub struct RteEthIntrConf {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DpdkError {
    Success = 0,
    InitError = 1,
    PortConfigError = 2,
    MemoryError = 3,
    RunningError = 4,
    NotInitialized = 5,
}

#[link(name = "rte_eal")]
#[link(name = "rte_mempool")]
#[link(name = "rte_mbuf")]
#[link(name = "rte_ethdev")]
extern "C" {
    fn rte_eal_init(argc: c_int, argv: *mut *mut c_char) -> c_int;
    fn rte_eal_cleanup() -> c_int;

    fn rte_pktmbuf_pool_create(
        name: *const c_char,
        n: c_uint,
        cache_size: c_uint,
        priv_size: c_ushort,
        data_room_size: c_ushort,
        socket_id: c_int,
    ) -> *mut RteMempool;

    fn rte_eth_dev_is_valid_port(port_id: c_ushort) -> c_int;
    fn rte_eth_dev_configure(
        port_id: c_ushort,
        nb_rx_queue: c_ushort,
        nb_tx_queue: c_ushort,
        eth_conf: *const c_void,
    ) -> c_int;
    fn rte_eth_rx_queue_setup(
        port_id: c_ushort,
        rx_queue_id: c_ushort,
        nb_rx_desc: c_ushort,
        socket_id: c_int,
        rx_conf: *const c_void,
        mb_pool: *mut RteMempool,
    ) -> c_int;
    fn rte_eth_tx_queue_setup(
        port_id: c_ushort,
        tx_queue_id: c_ushort,
        nb_tx_desc: c_ushort,
        socket_id: c_int,
        tx_conf: *const c_void,
    ) -> c_int;
    fn rte_eth_dev_start(port_id: c_ushort) -> c_int;
    fn rte_eth_promiscuous_enable(port_id: c_ushort) -> c_int;
    fn rte_eth_dev_stop(port_id: c_ushort) -> c_int;
    fn rte_eth_dev_close(port_id: c_ushort) -> c_int;

    fn rte_eth_rx_burst(
        port_id: c_ushort,
        queue_id: c_ushort,
        rx_pkts: *mut *mut RteMbuf,
        nb_pkts: c_ushort,
    ) -> c_ushort;
    fn rte_eth_tx_burst(
        port_id: c_ushort,
        queue_id: c_ushort,
        tx_pkts: *mut *mut RteMbuf,
        nb_pkts: c_ushort,
    ) -> c_ushort;

    fn rte_pktmbuf_free(m: *mut RteMbuf);
    fn rte_pktmbuf_mtod(m: *const RteMbuf, t: *const c_void) -> *mut c_void;
    fn rte_pktmbuf_data_len(m: *const RteMbuf) -> c_ushort;
    fn rte_eth_dev_socket_id(port_id: c_ushort) -> c_int;

    fn dpdk_extract_packet_data(
        pkt: *const RteMbuf,
        src_ip_out: *mut *mut u8,
        src_ip_len_out: *mut u32,
        dst_ip_out: *mut *mut u8,
        dst_ip_len_out: *mut u32,
        src_port_out: *mut u16,
        dst_port_out: *mut u16,
        data_out: *mut *mut u8,
        data_len_out: *mut u32,
    ) -> c_int;
}

pub struct DpdkWrapper {
    config: DpdkConfig,
    mbuf_pool: *mut RteMempool,
    initialized: bool,
    running: Arc<AtomicBool>,
    worker_threads: Vec<JoinHandle<()>>,
}

pub type PacketDataHandler = Arc<dyn Fn(u16, &PacketData) + Send + Sync + 'static>;

impl DpdkWrapper {
    pub fn new(config: DpdkConfig) -> Self {
        DpdkWrapper {
            config,
            mbuf_pool: ptr::null_mut(),
            initialized: false,
            running: Arc::new(AtomicBool::new(false)),
            worker_threads: Vec::new(),
        }
    }

    pub fn init(&mut self, args: &[String]) -> Result<(), DpdkError> {
        if self.initialized {
            return Ok(());
        }

        let mut eal_args = Vec::new();

        eal_args.extend_from_slice(args);

        if self.config.use_huge_pages {
            eal_args.push("--in-memory".to_string());

            if let Some(socket_mem) = &self.config.socket_mem {
                let socket_mem_arg = format!(
                    "--socket-mem={}",
                    socket_mem
                        .iter()
                        .map(|m| m.to_string())
                        .collect::<Vec<String>>()
                        .join(",")
                );
                eal_args.push(socket_mem_arg);
            }

            if let Some(huge_dir) = &self.config.huge_dir {
                eal_args.push(format!("--huge-dir={}", huge_dir));
            }

            eal_args.push("--huge-unlink".to_string());
        }

        let c_args: Vec<CString> = eal_args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect();

        let mut c_argv: Vec<*mut c_char> = c_args
            .iter()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect();

        let ret: i32 = unsafe { rte_eal_init(c_args.len() as c_int, c_argv.as_mut_ptr()) };

        if ret < 0 {
            return Err(DpdkError::InitError);
        }

        let socket_id = if self.config.use_numa_on_socket {
            unsafe {
                let port_socket = rte_eth_dev_socket_id(self.config.port_id);
                if port_socket >= 0 {
                    port_socket
                } else {
                    -1
                }
            }
        } else {
            -1
        };

        let pool_name: CString = CString::new("mbuf_pool").unwrap();
        self.mbuf_pool = unsafe {
            rte_pktmbuf_pool_create(
                pool_name.as_ptr(),
                self.config.num_mbufs,
                self.config.mbuf_cache_size,
                0,
                self.config.data_room_size,
                socket_id,
            )
        };

        if self.mbuf_pool.is_null() {
            unsafe { rte_eal_cleanup() };
            return Err(DpdkError::MemoryError);
        }

        self.initialized = true;
        Ok(())
    }

    pub fn configure_port(&self) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id: u16 = self.config.port_id;

        let is_valid: i32 = unsafe { rte_eth_dev_is_valid_port(port_id) };
        if is_valid == 0 {
            return Err(DpdkError::PortConfigError);
        }

        let mut eth_conf: RteEthConf = default_eth_config();

        let enable_rss: bool = self.config.enable_rss && self.config.num_rx_queues > 1;
        if enable_rss {
            eth_conf.rxmode.mq_mode = ETH_MQ_RX_RSS;

            eth_conf.rx_adv_conf.rss_conf.rss_hf = self.config.rss_hf;

            if let Some(ref key) = self.config.rss_key {
                eth_conf.rx_adv_conf.rss_conf.rss_key = key.as_ptr() as *mut u8;
                eth_conf.rx_adv_conf.rss_conf.rss_key_len = key.len() as u8;
            }
        }

        let ret: i32 = unsafe {
            rte_eth_dev_configure(
                port_id,
                self.config.num_rx_queues,
                self.config.num_tx_queues,
                &eth_conf as *const RteEthConf as *const c_void,
            )
        };

        if ret < 0 {
            return Err(DpdkError::PortConfigError);
        }

        for q in 0..self.config.num_rx_queues {
            let ret: i32 = unsafe {
                rte_eth_rx_queue_setup(
                    port_id,
                    q,
                    self.config.rx_ring_size as c_ushort,
                    -1,
                    ptr::null(),
                    self.mbuf_pool,
                )
            };

            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        for q in 0..self.config.num_tx_queues {
            let ret: i32 = unsafe {
                rte_eth_tx_queue_setup(
                    port_id,
                    q,
                    self.config.tx_ring_size as c_ushort,
                    -1,
                    ptr::null(),
                )
            };

            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        let ret: i32 = unsafe { rte_eth_dev_start(port_id) };
        if ret < 0 {
            return Err(DpdkError::PortConfigError);
        }

        if self.config.promiscuous {
            let ret = unsafe { rte_eth_promiscuous_enable(port_id) };
            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        Ok(())
    }

    pub fn start_processing(&mut self, queue_handler: PacketDataHandler) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id: u16 = self.config.port_id;
        let burst_size: u32 = self.config.burst_size;
        let running: Arc<AtomicBool> = self.running.clone();
        let num_queues: u16 = self.config.num_rx_queues;
        let use_affinity: bool = self.config.use_cpu_affinity;

        let packet_pool: Arc<PacketDataPool> =
            Arc::new(PacketDataPool::new(burst_size as usize * 4));

        running.store(true, Ordering::SeqCst);

        let core_ids: Arc<Vec<core_affinity::CoreId>> = Arc::new(if use_affinity {
            core_affinity::get_core_ids().unwrap_or_default()
        } else {
            Vec::new()
        });

        for queue_id in 0..num_queues {
            let queue_handler: Arc<dyn Fn(u16, &PacketData) + Send + Sync> = queue_handler.clone();
            let running_clone: Arc<AtomicBool> = running.clone();
            let core_ids_clone: Arc<Vec<core_affinity::CoreId>> = core_ids.clone();
            let packet_pool_clone: Arc<PacketDataPool> = packet_pool.clone();

            let thread_handle: JoinHandle<()> = std::thread::spawn(move || {
                if use_affinity && !core_ids_clone.is_empty() {
                    let core_index: usize = (queue_id as usize) % core_ids_clone.len();
                    if let Some(core_id) = core_ids_clone.get(core_index) {
                        core_affinity::set_for_current(*core_id);
                    }
                }

                let mut rx_pkts: Vec<*mut RteMbuf> = vec![ptr::null_mut(); burst_size as usize];

                while running_clone.load(Ordering::SeqCst) {
                    let nb_rx: u16 = unsafe {
                        rte_eth_rx_burst(
                            port_id,
                            queue_id,
                            rx_pkts.as_mut_ptr(),
                            burst_size as c_ushort,
                        )
                    };

                    for i in 0..nb_rx as usize {
                        let pkt: *mut RteMbuf = rx_pkts[i];

                        let mut src_ip_ptr: *mut u8 = ptr::null_mut();
                        let mut src_ip_len: u32 = 0;
                        let mut dst_ip_ptr: *mut u8 = ptr::null_mut();
                        let mut dst_ip_len: u32 = 0;
                        let mut src_port: u16 = 0;
                        let mut dst_port: u16 = 0;
                        let mut data_ptr: *mut u8 = ptr::null_mut();
                        let mut data_len: u32 = 0;

                        let ret: i32 = unsafe {
                            dpdk_extract_packet_data(
                                pkt,
                                &mut src_ip_ptr,
                                &mut src_ip_len,
                                &mut dst_ip_ptr,
                                &mut dst_ip_len,
                                &mut src_port,
                                &mut dst_port,
                                &mut data_ptr,
                                &mut data_len,
                            )
                        };

                        if ret == 0 && !data_ptr.is_null() && data_len > 0 {
                            let mut packet: PacketData = packet_pool_clone.acquire();

                            packet.source_port = src_port;
                            packet.dest_port = dst_port;
                            packet.queue_id = queue_id;

                            packet.source_ip_ptr = src_ip_ptr;
                            packet.source_ip_len = src_ip_len as usize;
                            packet.dest_ip_ptr = dst_ip_ptr;
                            packet.dest_ip_len = dst_ip_len as usize;
                            packet.data_ptr = data_ptr;
                            packet.data_len = data_len as usize;

                            packet.mbuf_ptr = pkt;

                            queue_handler(queue_id, &packet);

                            unsafe { rte_pktmbuf_free(packet.mbuf_ptr) };
                            packet_pool_clone.release(packet);
                        } else {
                            unsafe { rte_pktmbuf_free(pkt) };
                        }
                    }
                }
            });

            self.worker_threads.push(thread_handle);
        }

        Ok(())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        while let Some(handle) = self.worker_threads.pop() {
            let _ = handle.join();
        }
    }

    pub fn cleanup(&mut self) {
        if !self.initialized {
            return;
        }

        self.stop();

        unsafe {
            rte_eth_dev_stop(self.config.port_id);
            rte_eth_dev_close(self.config.port_id);
            rte_eal_cleanup();
        }

        self.initialized = false;
    }
}

impl Drop for DpdkWrapper {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn default_eth_config() -> RteEthConf {
    RteEthConf {
        rxmode: RteEthRxMode {
            mq_mode: 0,
            max_rx_pkt_len: 0,
            split_hdr_size: 0,
            offloads: 0,
        },
        txmode: RteEthTxMode {
            mq_mode: 0,
            pvid: 0,
            offloads: 0,
        },
        lpbk_mode: 0,
        rx_adv_conf: RteEthRxAdvConf {
            rss_conf: RteEthRssConf {
                rss_key: ptr::null_mut(),
                rss_key_len: 0,
                rss_hf: 0,
            },
        },
        tx_adv_conf: RteEthTxAdvConf {},
        dcb_capability_en: 0,
        fdir_conf: RteEthFdirConf {},
        intr_conf: RteEthIntrConf {},
    }
}

pub fn default_dpdk_config() -> DpdkConfig {
    DpdkConfig {
        port_id: 0,
        num_rx_queues: 4,
        num_tx_queues: 4,
        promiscuous: true,
        rx_ring_size: 1024,
        tx_ring_size: 1024,
        num_mbufs: 8191,
        mbuf_cache_size: 250,
        burst_size: 32,
        enable_rss: true,
        rss_hf: ETH_RSS_NONFRAG_IPV4_TCP | ETH_RSS_NONFRAG_IPV4_UDP | ETH_RSS_L4_DST_ONLY,
        use_cpu_affinity: true,
        rss_key: None,
        use_huge_pages: true,
        socket_mem: Some(vec![1024, 1024]),
        huge_dir: None,
        data_room_size: 2048,
        use_numa_on_socket: true,
    }
}
