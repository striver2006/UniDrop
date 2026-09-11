package registry

import (
	"sync"
	"time"

	"github.com/coder/websocket"
	"github.com/unidrop/unidrop-server/internal/protocol"
)

// DeviceSession represents an authenticated active client connection.
type DeviceSession struct {
	AccountID   string
	DeviceID    string
	Hostname    string
	OSType      string
	AppVersion  string
	RemoteIP    string
	ConnectedAt time.Time
	LastPingAt  time.Time

	ControlWS *websocket.Conn
	SendChan  chan []byte
	Closed    chan struct{}
	closeOnce sync.Once
}

// NewDeviceSession constructs a new DeviceSession.
func NewDeviceSession(accountID, deviceID, hostname, osType, appVersion, remoteIP string, ws *websocket.Conn) *DeviceSession {
	now := time.Now()
	return &DeviceSession{
		AccountID:   accountID,
		DeviceID:    deviceID,
		Hostname:    hostname,
		OSType:      osType,
		AppVersion:  appVersion,
		RemoteIP:    remoteIP,
		ConnectedAt: now,
		LastPingAt:  now,
		ControlWS:   ws,
		SendChan:    make(chan []byte, 256),
		Closed:      make(chan struct{}),
	}
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
		// Queue full, client is slow
		return false
	}
}

// TouchPing updates the last heartbeat time.
func (s *DeviceSession) TouchPing(now time.Time) {
	s.LastPingAt = now
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
