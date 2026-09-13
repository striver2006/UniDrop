package config

import (
	"os"
	"strconv"
)

// Config stores application configuration settings.
//
// Every field here must be read somewhere. Two former fields, MaxItemsPerOffer
// and HeartbeatInterval, were defined and assigned but never read by anything:
// the offer limit that actually applied was a bare 1000 literal in
// control_ws.go, so editing the config had no effect and produced no warning
// either (an exported struct field is never reported as unused). The offer
// limit now lives in internal/limits; the heartbeat interval is the client's
// decision, and the server only needs the timeout it sweeps against.
type Config struct {
	ListenAddr       string
	PSKSecret        string
	STUNAddr         string
	HeartbeatTimeout int // seconds
}

// Load loads configuration from environment variables with sensible defaults.
//
// Transfer limits are deliberately not read here — see internal/limits, which
// owns both their defaults and their parsing.
func Load() *Config {
	cfg := &Config{
		ListenAddr:       ":8080",
		PSKSecret:        "default-insecure-psk-change-me",
		STUNAddr:         ":3478",
		HeartbeatTimeout: 45,
	}

	if addr := os.Getenv("UNIDROP_LISTEN_ADDR"); addr != "" {
		cfg.ListenAddr = addr
	}
	if psk := os.Getenv("UNIDROP_PSK_SECRET"); psk != "" {
		cfg.PSKSecret = psk
	}
	if stun := os.Getenv("UNIDROP_STUN_ADDR"); stun != "" {
		cfg.STUNAddr = stun
	}
	if timeoutStr := os.Getenv("UNIDROP_HEARTBEAT_TIMEOUT"); timeoutStr != "" {
		if val, err := strconv.Atoi(timeoutStr); err == nil && val > 0 {
			cfg.HeartbeatTimeout = val
		}
	}

	return cfg
}
