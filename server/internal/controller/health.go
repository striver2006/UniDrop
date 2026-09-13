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
//
// Every field here must be read somewhere, for the same reason config.Config
// carries that rule: TotalOffers and RetransmitsCount used to sit in this
// struct, were assigned zero at startup and then never read or written by
// anything, and an exported struct field is never reported as unused. They have
// been removed rather than wired up — nothing wanted them.
//
// The counters use atomic.Uint64 rather than a bare uint64 plus package-level
// atomic.AddUint64 calls, so that the only way to touch them is the safe one.
type MetricsTracker struct {
	StartTime         time.Time
	TotalRelayedBytes atomic.Uint64

	// Handshake rejections, split by the closed reason set exposed as a
	// Prometheus label. These are the only automated signal for how much real
	// traffic the identifier validation introduced in auth/identity.go is
	// turning away, which matters because that validation is a breaking change
	// for anyone who had blanked out their account field.
	AuthRejectedInvalidAccount atomic.Uint64
	AuthRejectedInvalidDevice  atomic.Uint64
	AuthRejectedUnauthorized   atomic.Uint64
}

var Metrics = &MetricsTracker{
	StartTime: time.Now(),
}

// HealthHandler returns JSON health status.
//
// Note what is absent: the list of account IDs. Both this endpoint and
// /metrics are served without authentication (see main.go), so emitting the
// names would hand every account in the deployment to anyone who can issue a
// GET. The aggregates below answer the operational question without naming
// anyone.
func HealthHandler(reg *registry.DeviceRegistry, rm *relay.RelayManager) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		status := map[string]any{
			"status":                  "ok",
			"uptime_seconds":          int64(time.Since(Metrics.StartTime).Seconds()),
			"connected_devices":       reg.Count(),
			"online_accounts":         reg.CountAccounts(),
			"max_devices_per_account": reg.MaxDevicesPerAccount(),
			"active_pipes":            rm.Count(),
			"max_pipes_per_account":   rm.MaxPipesInUsePerAccount(),
			"goroutines":              runtime.NumGoroutine(),
		}
		_ = json.NewEncoder(w).Encode(status)
	}
}

// MetricsHandler returns Prometheus-formatted metrics.
//
// There is deliberately no account_id label anywhere in this output, and there
// must never be one. account_id is an unauthenticated, client-reported string:
// a single client can mint an unbounded number of distinct values just by
// reconnecting, and every one of them would become a permanent time series in
// whatever scrapes this. That turns /metrics into a memory amplification vector
// against both this process and the scraper. DESIGN.md once specified
// unidrop_connected_devices{account_id, os_type}; that specification was
// harmful and has been revised.
//
// The two "max per account" gauges exist to answer the question an operator
// actually has — "is one account eating the whole box?" — with a single number
// instead of one series per account. If you are tempted to add the label
// because the aggregate feels coarse, re-read the paragraph above.
//
// The {reason} label on the rejection counter is a different thing and is safe:
// it is a closed enum defined in code (three values), so its cardinality is
// bounded by construction rather than by what a client sends. It is not a
// precedent for labelling anything by account.
func MetricsHandler(reg *registry.DeviceRegistry, rm *relay.RelayManager) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		fmt.Fprintf(w, "# HELP unidrop_online_devices Currently connected online devices\n")
		fmt.Fprintf(w, "# TYPE unidrop_online_devices gauge\n")
		fmt.Fprintf(w, "unidrop_online_devices %d\n\n", reg.Count())

		fmt.Fprintf(w, "# HELP unidrop_online_accounts Currently connected distinct accounts\n")
		fmt.Fprintf(w, "# TYPE unidrop_online_accounts gauge\n")
		fmt.Fprintf(w, "unidrop_online_accounts %d\n\n", reg.CountAccounts())

		fmt.Fprintf(w, "# HELP unidrop_max_devices_per_account Devices held by the single busiest account\n")
		fmt.Fprintf(w, "# TYPE unidrop_max_devices_per_account gauge\n")
		fmt.Fprintf(w, "unidrop_max_devices_per_account %d\n\n", reg.MaxDevicesPerAccount())

		fmt.Fprintf(w, "# HELP unidrop_active_relay_pipes Currently active data relay pipes\n")
		fmt.Fprintf(w, "# TYPE unidrop_active_relay_pipes gauge\n")
		fmt.Fprintf(w, "unidrop_active_relay_pipes %d\n\n", rm.Count())

		fmt.Fprintf(w, "# HELP unidrop_max_pipes_per_account Relay pipes held by the single busiest account\n")
		fmt.Fprintf(w, "# TYPE unidrop_max_pipes_per_account gauge\n")
		fmt.Fprintf(w, "unidrop_max_pipes_per_account %d\n\n", rm.MaxPipesInUsePerAccount())

		fmt.Fprintf(w, "# HELP unidrop_relayed_bytes_total Total bytes relayed\n")
		fmt.Fprintf(w, "# TYPE unidrop_relayed_bytes_total counter\n")
		fmt.Fprintf(w, "unidrop_relayed_bytes_total %d\n\n", Metrics.TotalRelayedBytes.Load())

		fmt.Fprintf(w, "# HELP unidrop_auth_rejected_total Handshakes refused, by reason\n")
		fmt.Fprintf(w, "# TYPE unidrop_auth_rejected_total counter\n")
		fmt.Fprintf(w, "unidrop_auth_rejected_total{reason=\"invalid_account_id\"} %d\n",
			Metrics.AuthRejectedInvalidAccount.Load())
		fmt.Fprintf(w, "unidrop_auth_rejected_total{reason=\"invalid_device_id\"} %d\n",
			Metrics.AuthRejectedInvalidDevice.Load())
		fmt.Fprintf(w, "unidrop_auth_rejected_total{reason=\"unauthorized\"} %d\n\n",
			Metrics.AuthRejectedUnauthorized.Load())
	}
}
