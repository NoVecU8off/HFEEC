use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_ushort};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use core_affinity;
use crossbeam::queue::ArrayQueue;

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

#[repr(C, align(64))]
pub struct PacketData {
    pub source_port: u16,
    pub dest_port: u16,
    pub queue_id: u16,
    pub source_ip: String,
    pub dest_ip: String,
    pub data: Vec<u8>,
}

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

    fn dpdk_extract_packet_data(
        pkt: *const RteMbuf,
        src_ip_out: *mut c_char,
        dst_ip_out: *mut c_char,
        src_port_out: *mut c_ushort,
        dst_port_out: *mut c_ushort,
        data_out: *mut *mut u8,
        data_len_out: *mut c_uint,
    ) -> c_int;
}

#[derive(Copy, Clone)]
struct PacketDataPtr(*mut PacketData);

unsafe impl Send for PacketDataPtr {}
unsafe impl Sync for PacketDataPtr {}

impl PacketDataPtr {
    fn new(ptr: *mut PacketData) -> Self {
        PacketDataPtr(ptr)
    }

    fn get(&self) -> *mut PacketData {
        self.0
    }
}

pub struct PacketDataPool {
    queue: Arc<ArrayQueue<PacketDataPtr>>,
    _storage: Vec<Box<PacketData>>,
}

impl PacketDataPool {
    pub fn new(capacity: usize) -> Self {
        let queue = Arc::new(ArrayQueue::new(capacity));
        let mut _storage = Vec::with_capacity(capacity);

        for _ in 0..capacity {
            let mut data = Box::new(PacketData {
                source_port: 0,
                dest_port: 0,
                queue_id: 0,
                source_ip: String::with_capacity(16),
                dest_ip: String::with_capacity(16),
                data: Vec::with_capacity(2048),
            });

            let ptr = &mut *data as *mut PacketData;
            if let Err(_) = queue.push(PacketDataPtr::new(ptr)) {
                panic!("Failed to push to packet pool queue - this should never happen");
            };
            _storage.push(data);
        }

        Self { queue, _storage }
    }

    pub fn acquire(&self) -> Option<PacketDataHandle> {
        self.queue.pop().map(|ptr_wrapper| PacketDataHandle {
            data: unsafe { &mut *ptr_wrapper.get() },
            pool: self.queue.clone(),
            ptr_wrapper,
        })
    }
}

pub struct PacketDataHandle<'a> {
    pub data: &'a mut PacketData,
    pool: Arc<ArrayQueue<PacketDataPtr>>,
    ptr_wrapper: PacketDataPtr,
}

impl<'a> Drop for PacketDataHandle<'a> {
    fn drop(&mut self) {
        self.data.source_ip.clear();
        self.data.dest_ip.clear();
        self.data.data.clear();

        let _ = self.pool.push(self.ptr_wrapper);
    }
}

pub struct DpdkWrapper {
    config: DpdkConfig,
    mbuf_pool: *mut RteMempool,
    initialized: bool,
    running: Arc<AtomicBool>,
    worker_threads: Vec<JoinHandle<()>>,
}

pub type QueueSpecificHandler = Arc<dyn Fn(u16, &PacketData) + Send + Sync + 'static>;

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

        let c_args: Vec<CString> = args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect();

        let mut c_argv: Vec<*mut c_char> = c_args
            .iter()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect();

        let ret = unsafe { rte_eal_init(c_args.len() as c_int, c_argv.as_mut_ptr()) };

        if ret < 0 {
            return Err(DpdkError::InitError);
        }

        let pool_name = CString::new("mbuf_pool").unwrap();
        self.mbuf_pool = unsafe {
            rte_pktmbuf_pool_create(
                pool_name.as_ptr(),
                self.config.num_mbufs,
                self.config.mbuf_cache_size,
                0,
                0,
                -1,
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

        let port_id = self.config.port_id;

        let is_valid = unsafe { rte_eth_dev_is_valid_port(port_id) };
        if is_valid == 0 {
            return Err(DpdkError::PortConfigError);
        }

        let mut eth_conf = default_eth_config();

        let enable_rss = self.config.enable_rss && self.config.num_rx_queues > 1;
        if enable_rss {
            eth_conf.rxmode.mq_mode = ETH_MQ_RX_RSS;

            eth_conf.rx_adv_conf.rss_conf.rss_hf = self.config.rss_hf;

            if let Some(ref key) = self.config.rss_key {
                eth_conf.rx_adv_conf.rss_conf.rss_key = key.as_ptr() as *mut u8;
                eth_conf.rx_adv_conf.rss_conf.rss_key_len = key.len() as u8;
            }
        }

        let ret = unsafe {
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
            let ret = unsafe {
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
            let ret = unsafe {
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

        let ret = unsafe { rte_eth_dev_start(port_id) };
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

    pub fn start_processing_with_queue_handlers(
        &mut self,
        queue_handlers: QueueSpecificHandler,
    ) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id: u16 = self.config.port_id;
        let burst_size: u32 = self.config.burst_size;
        let running: Arc<AtomicBool> = self.running.clone();
        let num_queues: u16 = self.config.num_rx_queues;
        let use_affinity: bool = self.config.use_cpu_affinity;

        let packet_pool = Arc::new(PacketDataPool::new(burst_size as usize * 4));

        running.store(true, Ordering::SeqCst);

        let core_ids: Arc<Vec<core_affinity::CoreId>> = Arc::new(if use_affinity {
            core_affinity::get_core_ids().unwrap_or_default()
        } else {
            Vec::new()
        });

        for queue_id in 0..num_queues {
            let queue_handler = queue_handlers.clone();
            let running_clone = running.clone();
            let core_ids_clone = core_ids.clone();
            let packet_pool_clone = packet_pool.clone();

            let thread_handle: JoinHandle<()> = std::thread::spawn(move || {
                if use_affinity && !core_ids_clone.is_empty() {
                    let core_index: usize = (queue_id as usize) % core_ids_clone.len();
                    if let Some(core_id) = core_ids_clone.get(core_index) {
                        core_affinity::set_for_current(core_id.clone());
                    }
                }

                let mut rx_pkts: Vec<*mut RteMbuf> = vec![ptr::null_mut(); burst_size as usize];

                let mut src_ip_buf = [0u8; 64];
                let mut dst_ip_buf = [0u8; 64];

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

                        let src_ip_ptr: *mut i8 = src_ip_buf.as_mut_ptr() as *mut c_char;
                        let dst_ip_ptr: *mut i8 = dst_ip_buf.as_mut_ptr() as *mut c_char;
                        let mut src_port: c_ushort = 0;
                        let mut dst_port: c_ushort = 0;
                        let mut data_ptr: *mut u8 = ptr::null_mut();
                        let mut data_len: c_uint = 0;

                        let ret: i32 = unsafe {
                            dpdk_extract_packet_data(
                                pkt,
                                src_ip_ptr,
                                dst_ip_ptr,
                                &mut src_port,
                                &mut dst_port,
                                &mut data_ptr,
                                &mut data_len,
                            )
                        };

                        if ret == 0 && !data_ptr.is_null() && data_len > 0 {
                            if let Some(packet_handle) = packet_pool_clone.acquire() {
                                packet_handle.data.source_port = src_port;
                                packet_handle.data.dest_port = dst_port;
                                packet_handle.data.queue_id = queue_id;

                                let src_ip_str =
                                    unsafe { CStr::from_ptr(src_ip_ptr) }.to_string_lossy();
                                packet_handle.data.source_ip.push_str(&src_ip_str);

                                let dst_ip_str =
                                    unsafe { CStr::from_ptr(dst_ip_ptr) }.to_string_lossy();
                                packet_handle.data.dest_ip.push_str(&dst_ip_str);

                                let data_slice =
                                    unsafe { slice::from_raw_parts(data_ptr, data_len as usize) };
                                packet_handle.data.data.extend_from_slice(data_slice);

                                queue_handler(queue_id, packet_handle.data);
                            }
                        }

                        unsafe { rte_pktmbuf_free(pkt) };
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
    }
}
