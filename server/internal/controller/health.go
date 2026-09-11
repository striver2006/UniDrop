package controller

import (
	"encoding/json"
	"fmt"
	"net/http"
	"runtime"
	"sync/atomic"
	"time"

	"github.com/unidrop/unidrop-server/internal/registry"
	"github.com/unidrop/unidrop-server/internal/relay"
)

// MetricsTracker records basic runtime statistics.
type MetricsTracker struct {
	StartTime         time.Time
	TotalRelayedBytes uint64
	TotalOffers       uint64
	RetransmitsCount  uint64
}

var Metrics = &MetricsTracker{
	StartTime: time.Now(),
}

// HealthHandler returns JSON health status.
func HealthHandler(reg *registry.DeviceRegistry, rm *relay.RelayManager) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		status := map[string]any{
			"status":            "ok",
			"uptime_seconds":    int64(time.Since(Metrics.StartTime).Seconds()),
			"connected_devices": reg.Count(),
			"active_pipes":      rm.Count(),
			"goroutines":        runtime.NumGoroutine(),
		}
		_ = json.NewEncoder(w).Encode(status)
	}
}

// MetricsHandler returns Prometheus-formatted metrics.
func MetricsHandler(reg *registry.DeviceRegistry, rm *relay.RelayManager) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		fmt.Fprintf(w, "# HELP unidrop_online_devices Currently connected online devices\n")
		fmt.Fprintf(w, "# TYPE unidrop_online_devices gauge\n")
		fmt.Fprintf(w, "unidrop_online_devices %d\n\n", reg.Count())

		fmt.Fprintf(w, "# HELP unidrop_active_relay_pipes Currently active data relay pipes\n")
		fmt.Fprintf(w, "# TYPE unidrop_active_relay_pipes gauge\n")
		fmt.Fprintf(w, "unidrop_active_relay_pipes %d\n\n", rm.Count())

		fmt.Fprintf(w, "# HELP unidrop_relayed_bytes_total Total bytes relayed\n")
		fmt.Fprintf(w, "# TYPE unidrop_relayed_bytes_total counter\n")
		fmt.Fprintf(w, "unidrop_relayed_bytes_total %d\n\n", atomic.LoadUint64(&Metrics.TotalRelayedBytes))
	}
}
