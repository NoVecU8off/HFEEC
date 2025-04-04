use std::os::raw::{c_uint, c_ushort};

/// Конфигурация DPDK с поддержкой NUMA
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
    pub use_rss: bool,
    pub rss_hf: u64,
    pub use_cpu_affinity: bool,
    pub rss_key: Option<Vec<u8>>,
    pub use_huge_pages: bool,
    pub socket_mem: Option<Vec<u32>>,
    pub huge_dir: Option<String>,
    pub data_room_size: c_ushort,
    pub use_numa_on_socket: bool,
    pub use_jumbo_frames: bool,
    pub max_rx_pkt_len: u32,
    pub use_hw_checksum: bool,
    pub use_flow_director: bool,
    pub use_tso: bool,
    pub use_lro: bool,
    pub use_udp_tso: bool,
    pub max_tso_segment_size: u16,
}

impl Default for DpdkConfig {
    fn default() -> Self {
        use crate::dpdk::ffi::{
            ETH_RSS_L4_DST_ONLY, ETH_RSS_NONFRAG_IPV4_TCP, ETH_RSS_NONFRAG_IPV4_UDP,
        };

        Self {
            port_id: 0,
            num_rx_queues: 4,
            num_tx_queues: 4,
            promiscuous: true,
            rx_ring_size: 1024,
            tx_ring_size: 1024,
            num_mbufs: 8191,
            mbuf_cache_size: 250,
            burst_size: 32,
            use_rss: true,
            rss_hf: ETH_RSS_NONFRAG_IPV4_TCP | ETH_RSS_NONFRAG_IPV4_UDP | ETH_RSS_L4_DST_ONLY,
            use_cpu_affinity: true,
            rss_key: None,
            use_huge_pages: true,
            socket_mem: Some(vec![1024, 1024]),
            huge_dir: None,
            data_room_size: 2048,
            use_numa_on_socket: true,
            use_jumbo_frames: false,
            max_rx_pkt_len: 1518,
            use_hw_checksum: true,
            use_flow_director: false,
            use_tso: false,
            use_lro: false,
            use_udp_tso: false,
            max_tso_segment_size: 1460, // Типичный размер MSS (MTU - заголовки TCP/IP)
        }
    }
}

impl DpdkConfig {
    /// Создает конфигурацию для работы с Jumbo Frames
    pub fn with_jumbo_frames(mut self, mtu: u32) -> Self {
        self.use_jumbo_frames = true;
        self.max_rx_pkt_len = mtu + 18; // Ethernet header (14) + VLAN tag (4)
        self.data_room_size = (self.max_rx_pkt_len + 128) as c_ushort; // Дополнительное пространство для заголовков
        self
    }

    /// Настраивает выделение памяти для указанного количества NUMA узлов
    pub fn with_numa_allocation(mut self, num_nodes: usize, mb_per_node: u32) -> Self {
        self.socket_mem = Some(vec![mb_per_node; num_nodes]);
        self.use_numa_on_socket = true;
        self
    }

    /// Отключает поддержку NUMA для тестирования
    pub fn without_numa(mut self) -> Self {
        self.use_numa_on_socket = false;
        self
    }

    /// Включает поддержку TCP Segmentation Offload (TSO)
    pub fn with_tso(mut self, max_segment_size: Option<u16>) -> Self {
        self.use_tso = true;
        if let Some(mss) = max_segment_size {
            self.max_tso_segment_size = mss;
        }
        self
    }

    /// Включает поддержку Large Receive Offload (LRO)
    pub fn with_lro(mut self) -> Self {
        self.use_lro = true;
        self
    }

    /// Включает поддержку UDP TSO (GSO) для фрагментированных UDP пакетов
    pub fn with_udp_tso(mut self, max_segment_size: Option<u16>) -> Self {
        self.use_udp_tso = true;
        if let Some(mss) = max_segment_size {
            self.max_tso_segment_size = mss;
        }
        self
    }
}

/// Создает конфигурацию DPDK с параметрами по умолчанию
pub fn default_dpdk_config() -> DpdkConfig {
    DpdkConfig::default()
}
