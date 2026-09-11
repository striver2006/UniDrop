package registry

import (
	"log/slog"
	"sync"
	"sync/atomic"
	"time"

	"github.com/coder/websocket"
	"github.com/unidrop/unidrop-server/internal/protocol"
)

// DeviceSession represents an authenticated active client connection.
type DeviceSession struct {
	AccountID    string
	DeviceID     string
	Hostname     string
	OSType       string
	AppVersion   string
	RemoteIP     string
	ConnectedAt  time.Time
	lastPingNano atomic.Int64

	ControlWS *websocket.Conn
	SendChan  chan []byte
	Closed    chan struct{}
	closeOnce sync.Once
}

// NewDeviceSession constructs a new DeviceSession.
func NewDeviceSession(accountID, deviceID, hostname, osType, appVersion, remoteIP string, ws *websocket.Conn) *DeviceSession {
	now := time.Now()
	s := &DeviceSession{
		AccountID:   accountID,
		DeviceID:    deviceID,
		Hostname:    hostname,
		OSType:      osType,
		AppVersion:  appVersion,
		RemoteIP:    remoteIP,
		ConnectedAt: now,
		ControlWS:   ws,
		SendChan:    make(chan []byte, 256),
		Closed:      make(chan struct{}),
	}
	s.lastPingNano.Store(now.UnixNano())
	return s
}

// Close gracefully closes the session and signals the write pump.
func (s *DeviceSession) Close() {
	s.closeOnce.Do(func() {
		close(s.Closed)
		if s.ControlWS != nil {
			_ = s.ControlWS.Close(websocket.StatusNormalClosure, "session closed")
		}
	})
}

// Send attempts to enqueue a message to the client. Returns false if channel is full or session closed.
func (s *DeviceSession) Send(msg []byte) bool {
	select {
	case <-s.Closed:
		return false
	case s.SendChan <- msg:
		return true
	default:
		slog.Warn("client send channel full, message dropped", "device_id", s.DeviceID)
		return false
	}
}

// TouchPing updates the last heartbeat time atomically.
func (s *DeviceSession) TouchPing(now time.Time) {
	s.lastPingNano.Store(now.UnixNano())
}

// GetLastPing returns the last heartbeat time.
func (s *DeviceSession) GetLastPing() time.Time {
	return time.Unix(0, s.lastPingNano.Load())
}

// ToOnlineDevice converts session into public protocol model.
func (s *DeviceSession) ToOnlineDevice() protocol.OnlineDevice {
	return protocol.OnlineDevice{
		DeviceID:   s.DeviceID,
		Hostname:   s.Hostname,
		OSType:     s.OSType,
		AppVersion: s.AppVersion,
		RemoteIP:   s.RemoteIP,
	}
}
