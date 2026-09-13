package main

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/unidrop/unidrop-server/internal/auth"
	"github.com/unidrop/unidrop-server/internal/config"
	"github.com/unidrop/unidrop-server/internal/controller"
	"github.com/unidrop/unidrop-server/internal/limits"
	"github.com/unidrop/unidrop-server/internal/registry"
	"github.com/unidrop/unidrop-server/internal/relay"
	"github.com/unidrop/unidrop-server/internal/stun"
)

func main() {
	// Initialize structured logger
	logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	}))
	slog.SetDefault(logger)

	cfg := config.Load()
	slog.Info("starting UniDrop Server", "listen", cfg.ListenAddr)
	if cfg.PSKSecret == "default-insecure-psk-change-me" {
		slog.Warn("SECURITY WARNING: Running with default insecure PSK secret! Set UNIDROP_PSK_SECRET environment variable for production (P1-3).")
	}

	verifier, err := auth.NewVerifier(cfg.PSKSecret)
	if err != nil {
		slog.Error("failed to initialize auth verifier", "error", err)
		os.Exit(1)
	}

	transferLimits := limits.FromEnv()
	limits.LogSummary(transferLimits)

	devRegistry := registry.NewDeviceRegistry()
	relayManager := relay.NewRelayManager()

	// Optional STUN service with graceful shutdown support (P2-9)
	var stunCloser io.Closer
	if cfg.STUNAddr != "" {
		var err error
		stunCloser, err = stun.StartSTUNServerWithCloser(cfg.STUNAddr)
		if err != nil {
			slog.Warn("STUN service failed to start", "addr", cfg.STUNAddr, "error", err)
		}
	}

	// Periodic cleanup worker (every 10 seconds)
	ticker := time.NewTicker(10 * time.Second)
	defer ticker.Stop()
	go func() {
		for range ticker.C {
			now := time.Now()
			// Sweep timed-out sessions (>45s)
			timedOut := devRegistry.SweepInactive(time.Duration(cfg.HeartbeatTimeout)*time.Second, now)
			if len(timedOut) > 0 {
				slog.Info("swept timed-out sessions", "count", len(timedOut))
			}
			// Sweep idle relay pipes (>60s)
			sweptPipes := relayManager.SweepIdlePipes(relay.DefaultPipeIdleTimeout, now)
			if len(sweptPipes) > 0 {
				slog.Info("swept idle relay pipes", "count", len(sweptPipes))
			}
		}
	}()

	// Register HTTP / WebSocket routes
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", controller.HealthHandler(devRegistry, relayManager))
	mux.HandleFunc("GET /metrics", controller.MetricsHandler(devRegistry, relayManager))
	mux.Handle("GET /ws/control", controller.NewControlWSHandler(verifier, devRegistry, relayManager, transferLimits))
	mux.Handle("GET /ws/data", controller.NewDataWSHandler(relayManager))

	server := &http.Server{
		Addr:         cfg.ListenAddr,
		Handler:      mux,
		ReadTimeout:  15 * time.Second,
		WriteTimeout: 15 * time.Second,
		IdleTimeout:  60 * time.Second,
	}

	// Server runner
	go func() {
		slog.Info("server listening", "addr", cfg.ListenAddr)
		if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			slog.Error("server listen failed", "error", err)
			os.Exit(1)
		}
	}()

	// Graceful shutdown on SIGINT / SIGTERM
	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)
	<-quit
	slog.Info("shutting down server...")

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := server.Shutdown(ctx); err != nil {
		slog.Error("forced server shutdown", "error", err)
	}
	if stunCloser != nil {
		if err := stunCloser.Close(); err != nil {
			slog.Warn("failed to close STUN server cleanly", "error", err)
		}
	}
	slog.Info("server exited gracefully")
}
