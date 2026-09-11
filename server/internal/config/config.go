package config

import (
	"os"
	"strconv"
)

// Config stores application configuration settings.
type Config struct {
	ListenAddr        string
	PSKSecret         string
	STUNAddr          string
	MaxItemsPerOffer  int
	HeartbeatInterval int // seconds
	HeartbeatTimeout  int // seconds
}

// Load loads configuration from environment variables with sensible defaults.
func Load() *Config {
	cfg := &Config{
		ListenAddr:        ":8080",
		PSKSecret:         "default-insecure-psk-change-me",
		STUNAddr:          ":3478",
		MaxItemsPerOffer:  1000,
		HeartbeatInterval: 15,
		HeartbeatTimeout:  45,
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
