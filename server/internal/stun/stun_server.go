package stun

import (
	"encoding/binary"
	"log/slog"
	"net"
)

// STUN Magic Cookie (RFC 5389 / RFC 8489)
const MagicCookie uint32 = 0x2112A442

// STUN Message Types
const (
	BindingRequest = 0x0001
	BindingResponse = 0x0101
)

// STUN Attributes
const (
	AttrXORMappedAddress = 0x0020
)

// StartSTUNServer starts a lightweight UDP STUN responder on the given address.
func StartSTUNServer(addr string) error {
	conn, err := net.ListenPacket("udp", addr)
	if err != nil {
		return err
	}
	slog.Info("STUN server listening", "addr", addr)

	go func() {
		defer conn.Close()
		buf := make([]byte, 1024)
		for {
			n, clientAddr, err := conn.ReadFrom(buf)
			if err != nil {
				return
			}
			if n < 20 {
				continue
			}

			msgType := binary.BigEndian.Uint16(buf[0:2])
			cookie := binary.BigEndian.Uint32(buf[4:8])

			if msgType == BindingRequest && cookie == MagicCookie {
				udpAddr, ok := clientAddr.(*net.UDPAddr)
				if !ok {
					continue
				}

				resp := buildBindingResponse(buf[8:20], udpAddr)
				_, _ = conn.WriteTo(resp, clientAddr)
			}
		}
	}()

	return nil
}

func buildBindingResponse(transactionID []byte, addr *net.UDPAddr) []byte {
	// 20 bytes STUN header + 12 bytes XOR-MAPPED-ADDRESS attribute
	resp := make([]byte, 32)
	binary.BigEndian.PutUint16(resp[0:2], BindingResponse)
	binary.BigEndian.PutUint16(resp[2:4], 12) // Attribute length
	binary.BigEndian.PutUint32(resp[4:8], MagicCookie)
	copy(resp[8:20], transactionID)

	// XOR-MAPPED-ADDRESS
	binary.BigEndian.PutUint16(resp[20:22], AttrXORMappedAddress)
	binary.BigEndian.PutUint16(resp[22:24], 8) // Value length
	resp[24] = 0x00                          // Reserved
	resp[25] = 0x01                          // IPv4 family

	// XOR Port
	xorPort := uint16(addr.Port) ^ uint16(MagicCookie>>16)
	binary.BigEndian.PutUint16(resp[26:28], xorPort)

	// XOR IPv4
	ip := addr.IP.To4()
	if ip != nil {
		xorIP := binary.BigEndian.Uint32(ip) ^ MagicCookie
		binary.BigEndian.PutUint32(resp[28:32], xorIP)
	}

	return resp
}
