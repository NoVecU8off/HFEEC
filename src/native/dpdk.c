#include <rte_eal.h>
#include <rte_ethdev.h>
#include <rte_mbuf.h>
#include <rte_ip.h>
#include <rte_tcp.h>
#include <rte_udp.h>
#include <rte_ether.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <arpa/inet.h>

/**
 * Извлекает информацию и данные из пакета DPDK для передачи в Rust
 * 
 * @param pkt Указатель на структуру пакета DPDK
 * @param src_ip_out Указатель на буфер для записи IP-адреса источника
 * @param dst_ip_out Указатель на буфер для записи IP-адреса назначения
 * @param src_port_out Указатель на переменную для записи порта источника
 * @param dst_port_out Указатель на переменную для записи порта назначения
 * @param data_out Указатель на переменную для указателя на данные пакета
 * @param data_len_out Указатель на переменную для длины данных
 * @return 0 в случае успеха, ненулевое значение в случае ошибки
 */
int dpdk_extract_packet_data(
    const struct rte_mbuf *pkt,
    uint8_t **src_ip_out,
    uint32_t *src_ip_len_out,
    uint8_t **dst_ip_out,
    uint32_t *dst_ip_len_out,
    uint16_t *src_port_out,
    uint16_t *dst_port_out,
    uint8_t **data_out,
    uint32_t *data_len_out
) {
    if (!pkt || !src_ip_out || !src_ip_len_out || !dst_ip_out || !dst_ip_len_out || 
        !src_port_out || !dst_port_out || !data_out || !data_len_out) {
        return -1;
    }
    
    *src_port_out = 0;
    *dst_port_out = 0;
    *data_out = NULL;
    *data_len_out = 0;
    *src_ip_out = NULL;
    *src_ip_len_out = 0;
    *dst_ip_out = NULL;
    *dst_ip_len_out = 0;
    
    struct rte_ether_hdr *eth_hdr = rte_pktmbuf_mtod(pkt, struct rte_ether_hdr *);
    
    if (rte_be_to_cpu_16(eth_hdr->ether_type) != RTE_ETHER_TYPE_IPV4) {
        return -2;
    }
    
    struct rte_ipv4_hdr *ip_hdr = (struct rte_ipv4_hdr *)(eth_hdr + 1);
    
    *src_ip_out = (uint8_t *)&ip_hdr->src_addr;
    *src_ip_len_out = sizeof(ip_hdr->src_addr);
    *dst_ip_out = (uint8_t *)&ip_hdr->dst_addr;
    *dst_ip_len_out = sizeof(ip_hdr->dst_addr);
    
    uint16_t payload_offset = 0;
    
    if (ip_hdr->next_proto_id == IPPROTO_TCP) {
        struct rte_tcp_hdr *tcp_hdr = (struct rte_tcp_hdr *)((uint8_t *)ip_hdr + 
                                    (ip_hdr->version_ihl & 0x0f) * 4);
        
        *src_port_out = rte_be_to_cpu_16(tcp_hdr->src_port);
        *dst_port_out = rte_be_to_cpu_16(tcp_hdr->dst_port);
        
        uint8_t tcp_header_size = ((tcp_hdr->data_off & 0xf0) >> 4) * 4;
        payload_offset = ((ip_hdr->version_ihl & 0x0f) * 4) + tcp_header_size;
    } 
    else if (ip_hdr->next_proto_id == IPPROTO_UDP) {
        struct rte_udp_hdr *udp_hdr = (struct rte_udp_hdr *)((uint8_t *)ip_hdr + 
                                    (ip_hdr->version_ihl & 0x0f) * 4);
        
        *src_port_out = rte_be_to_cpu_16(udp_hdr->src_port);
        *dst_port_out = rte_be_to_cpu_16(udp_hdr->dst_port);
        
        payload_offset = ((ip_hdr->version_ihl & 0x0f) * 4) + 8;
    }
    else {
        return -3;
    }
    
    uint16_t ip_total_length = rte_be_to_cpu_16(ip_hdr->total_length);
    uint32_t payload_length = 0;
    
    if (ip_total_length > payload_offset) {
        payload_length = ip_total_length - payload_offset;
    }
    else {
        return -4;
    }
    
    if (payload_length > 0) {
        uint8_t *payload = (uint8_t *)ip_hdr + payload_offset;
        
        *data_out = payload;
        *data_len_out = payload_length;
        
        return 0;
    }
    
    return -5;
}

/**
 * Создает новый пакет DPDK и заполняет его данными для отправки
 * 
 * @param mbuf_pool Пул памяти для создания пакета
 * @param src_ip IP-адрес источника
 * @param dst_ip IP-адрес назначения
 * @param src_port Порт источника
 * @param dst_port Порт назначения
 * @param data Указатель на данные для отправки
 * @param data_len Длина данных
 * @param use_tcp Использовать TCP (1) или UDP (0)
 * @return Указатель на созданный пакет или NULL в случае ошибки
 */
struct rte_mbuf* dpdk_create_packet(
    struct rte_mempool *mbuf_pool,
    const char *src_ip,
    const char *dst_ip,
    uint16_t src_port,
    uint16_t dst_port,
    const uint8_t *data,
    uint32_t data_len,
    int use_tcp
) {
    struct rte_mbuf *mbuf = rte_pktmbuf_alloc(mbuf_pool);
    if (mbuf == NULL) {
        return NULL;
    }
    
    uint16_t eth_hdr_size = sizeof(struct rte_ether_hdr);
    uint16_t ip_hdr_size = sizeof(struct rte_ipv4_hdr);
    uint16_t l4_hdr_size = use_tcp ? sizeof(struct rte_tcp_hdr) : sizeof(struct rte_udp_hdr);
    uint16_t total_hdr_size = eth_hdr_size + ip_hdr_size + l4_hdr_size;
    
    uint16_t total_size = total_hdr_size + data_len;
    
    char *payload = rte_pktmbuf_append(mbuf, total_size);
    if (payload == NULL) {
        rte_pktmbuf_free(mbuf);
        return NULL;
    }
    
    struct rte_ether_hdr *eth_hdr = (struct rte_ether_hdr *)payload;
    struct rte_ipv4_hdr *ip_hdr = (struct rte_ipv4_hdr *)(payload + eth_hdr_size);
    void *l4_hdr = payload + eth_hdr_size + ip_hdr_size;
    uint8_t *pkt_data = (uint8_t *)(payload + total_hdr_size);
    
    memset(&eth_hdr->dst_addr, 0xFF, RTE_ETHER_ADDR_LEN); 
    memset(&eth_hdr->src_addr, 0xAA, RTE_ETHER_ADDR_LEN); 
    eth_hdr->ether_type = rte_cpu_to_be_16(RTE_ETHER_TYPE_IPV4);
    
    memset(ip_hdr, 0, ip_hdr_size);
    ip_hdr->version_ihl = 0x45; 
    ip_hdr->type_of_service = 0;
    ip_hdr->total_length = rte_cpu_to_be_16(ip_hdr_size + l4_hdr_size + data_len);
    ip_hdr->packet_id = 0;
    ip_hdr->fragment_offset = 0;
    ip_hdr->time_to_live = 64; // TTL
    ip_hdr->next_proto_id = use_tcp ? IPPROTO_TCP : IPPROTO_UDP;
    
    inet_pton(AF_INET, src_ip, &ip_hdr->src_addr);
    inet_pton(AF_INET, dst_ip, &ip_hdr->dst_addr);
    
    if (data != NULL && data_len > 0) {
        memcpy(pkt_data, data, data_len);
    }

    mbuf->ol_flags |= RTE_MBUF_F_TX_IP_CKSUM;  
    mbuf->l2_len = sizeof(struct rte_ether_hdr);  
    mbuf->l3_len = sizeof(struct rte_ipv4_hdr);  

    ip_hdr->hdr_checksum = 0;

    if (use_tcp) {
        struct rte_tcp_hdr *tcp_hdr = (struct rte_tcp_hdr *)l4_hdr;
        memset(tcp_hdr, 0, sizeof(struct rte_tcp_hdr));
        
        tcp_hdr->src_port = rte_cpu_to_be_16(src_port);
        tcp_hdr->dst_port = rte_cpu_to_be_16(dst_port);
        tcp_hdr->data_off = 0x50;
        tcp_hdr->rx_win = rte_cpu_to_be_16(8192); 
        
        mbuf->ol_flags |= RTE_MBUF_F_TX_TCP_CKSUM;
        tcp_hdr->cksum = rte_ipv4_phdr_cksum(ip_hdr, mbuf->ol_flags);
    } else {
        struct rte_udp_hdr *udp_hdr = (struct rte_udp_hdr *)l4_hdr;
        memset(udp_hdr, 0, sizeof(struct rte_udp_hdr));
        
        udp_hdr->src_port = rte_cpu_to_be_16(src_port);
        udp_hdr->dst_port = rte_cpu_to_be_16(dst_port);
        udp_hdr->dgram_len = rte_cpu_to_be_16(l4_hdr_size + data_len);
        
        mbuf->ol_flags |= RTE_MBUF_F_TX_UDP_CKSUM;
        udp_hdr->dgram_cksum = rte_ipv4_phdr_cksum(ip_hdr, mbuf->ol_flags);
    }
    
    return mbuf;
}