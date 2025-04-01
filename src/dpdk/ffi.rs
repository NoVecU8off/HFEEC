use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_uint, c_ushort};

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

// RSS константы
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
    pub fn rte_eal_init(argc: c_int, argv: *mut *mut c_char) -> c_int;
    pub fn rte_eal_cleanup() -> c_int;

    pub fn rte_pktmbuf_pool_create(
        name: *const c_char,
        n: c_uint,
        cache_size: c_uint,
        priv_size: c_ushort,
        data_room_size: c_ushort,
        socket_id: c_int,
    ) -> *mut RteMempool;

    pub fn rte_eth_dev_is_valid_port(port_id: c_ushort) -> c_int;
    pub fn rte_eth_dev_configure(
        port_id: c_ushort,
        nb_rx_queue: c_ushort,
        nb_tx_queue: c_ushort,
        eth_conf: *const c_void,
    ) -> c_int;
    pub fn rte_eth_rx_queue_setup(
        port_id: c_ushort,
        rx_queue_id: c_ushort,
        nb_rx_desc: c_ushort,
        socket_id: c_int,
        rx_conf: *const c_void,
        mb_pool: *mut RteMempool,
    ) -> c_int;
    pub fn rte_eth_tx_queue_setup(
        port_id: c_ushort,
        tx_queue_id: c_ushort,
        nb_tx_desc: c_ushort,
        socket_id: c_int,
        tx_conf: *const c_void,
    ) -> c_int;
    pub fn rte_eth_dev_start(port_id: c_ushort) -> c_int;
    pub fn rte_eth_promiscuous_enable(port_id: c_ushort) -> c_int;
    pub fn rte_eth_dev_stop(port_id: c_ushort) -> c_int;
    pub fn rte_eth_dev_close(port_id: c_ushort) -> c_int;

    pub fn rte_eth_rx_burst(
        port_id: c_ushort,
        queue_id: c_ushort,
        rx_pkts: *mut *mut RteMbuf,
        nb_pkts: c_ushort,
    ) -> c_ushort;
    pub fn rte_eth_tx_burst(
        port_id: c_ushort,
        queue_id: c_ushort,
        tx_pkts: *mut *mut RteMbuf,
        nb_pkts: c_ushort,
    ) -> c_ushort;

    pub fn rte_pktmbuf_free(m: *mut RteMbuf);
    pub fn rte_pktmbuf_mtod(m: *const RteMbuf, t: *const c_void) -> *mut c_void;
    pub fn rte_pktmbuf_data_len(m: *const RteMbuf) -> c_ushort;
    pub fn rte_eth_dev_socket_id(port_id: c_ushort) -> c_int;

    pub fn dpdk_extract_packet_data(
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

/// Создает конфигурацию DPDK с параметрами по умолчанию
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
