// Package limits owns the server-side transfer limit set: its defaults, its
// environment parsing, and the single place an offer is checked against it.
//
// This package is the only source of truth for those five numbers. It reads
// the environment itself rather than going through internal/config so that
// the values exist in exactly one struct — routing them through Config first
// would mean the same five numbers live in two places, which is the very
// defect this round was opened to remove (config.MaxItemsPerOffer was defined,
// assigned, and never read, while a bare literal in control_ws.go decided
// the actual behaviour).
package limits

import (
	"fmt"
	"log/slog"
	"os"
	"strconv"

	"github.com/unidrop/unidrop-server/internal/protocol"
)

// Default limits. Byte counts are binary (1 MB = 1024*1024), matching the
// convention already used by the client's cache_max_size_mb.
const (
	DefaultMaxSingleFileBytes     int64 = 128 * 1024 * 1024 // 128 MB
	DefaultMaxTotalTransferBytes  int64 = 256 * 1024 * 1024 // 256 MB
	DefaultMaxClipboardImageBytes int64 = 64 * 1024 * 1024  // 64 MB
	DefaultMaxClipboardTextBytes  int64 = 4 * 1024 * 1024   // 4 MB
	DefaultMaxItemsPerOffer       int   = 64
	DefaultMaxConcurrentTransfers int   = 8
)

// Environment variable names.
const (
	EnvMaxSingleFileBytes     = "UNIDROP_MAX_SINGLE_FILE_BYTES"
	EnvMaxTotalTransferBytes  = "UNIDROP_MAX_TOTAL_TRANSFER_BYTES"
	EnvMaxClipboardImageBytes = "UNIDROP_MAX_CLIPBOARD_IMAGE_BYTES"
	EnvMaxClipboardTextBytes  = "UNIDROP_MAX_CLIPBOARD_TEXT_BYTES"
	EnvMaxItemsPerOffer       = "UNIDROP_MAX_ITEMS_PER_OFFER"
	EnvMaxConcurrentTransfers = "UNIDROP_MAX_CONCURRENT_TRANSFERS"
)

// Violation error codes sent to the client in TRANSFER_FAILURE.
const (
	CodeFileTooLarge      = "LIMIT_FILE_TOO_LARGE"
	CodeTotalTooLarge     = "LIMIT_TOTAL_TOO_LARGE"
	CodeImageTooLarge     = "LIMIT_IMAGE_TOO_LARGE"
	CodeTextTooLarge      = "LIMIT_TEXT_TOO_LARGE"
	CodeTooManyItems      = "LIMIT_TOO_MANY_ITEMS"
	CodeTooManyConcurrent = "LIMIT_TOO_MANY_CONCURRENT"
	CodeMalformedOffer    = "LIMIT_MALFORMED_OFFER"
)

// Violation describes a rejected offer. Message is user-facing Chinese text
// and always carries both the actual value and the limit: a user who only
// reads "文件太大" cannot tell what to change.
type Violation struct {
	Code    string
	Message string
}

func (v *Violation) Error() string { return v.Code + ": " + v.Message }

// Limits is an alias for the wire type. The struct itself lives in
// internal/protocol because it crosses the network; the rules that give it
// meaning live here.
type Limits = protocol.ServerLimits

// FromEnv builds the limit set from the environment.
//
// Each variable follows a three-state rule:
//
//	unset (or empty)   -> the default
//	parses and >= 0    -> taken as-is, where 0 means "no limit"
//	unparsable or < 0  -> the default, with a warning
//
// The third state is the one that matters. strconv's failure value is 0, and
// 0 here means "unlimited" — so a typo'd variable name or a value written as
// "2GB" would, with the obvious `v, _ := strconv.ParseInt(...)`, silently turn
// a limit off while the operator believes they just configured it. The warning
// is the only signal they will ever get, so it must not be skipped.
func FromEnv() Limits {
	return Limits{
		MaxSingleFileBytes:     envInt64(EnvMaxSingleFileBytes, DefaultMaxSingleFileBytes),
		MaxTotalTransferBytes:  envInt64(EnvMaxTotalTransferBytes, DefaultMaxTotalTransferBytes),
		MaxClipboardImageBytes: envInt64(EnvMaxClipboardImageBytes, DefaultMaxClipboardImageBytes),
		MaxClipboardTextBytes:  envInt64(EnvMaxClipboardTextBytes, DefaultMaxClipboardTextBytes),
		MaxItemsPerOffer:       envInt(EnvMaxItemsPerOffer, DefaultMaxItemsPerOffer),
		MaxConcurrentTransfers: envInt(EnvMaxConcurrentTransfers, DefaultMaxConcurrentTransfers),
	}
}

// envInt64 is used for byte counts. ParseInt with an explicit 64-bit width is
// required rather than strconv.Atoi: Atoi returns int, which is 32 bits on a
// 32-bit build, and these values routinely exceed 2 GiB once an operator
// raises them.
func envInt64(name string, def int64) int64 {
	raw, ok := os.LookupEnv(name)
	if !ok || raw == "" {
		return def
	}
	v, err := strconv.ParseInt(raw, 10, 64)
	if err != nil || v < 0 {
		slog.Warn("invalid transfer limit, keeping default",
			"var", name, "value", raw, "default", def)
		return def
	}
	return v
}

func envInt(name string, def int) int {
	raw, ok := os.LookupEnv(name)
	if !ok || raw == "" {
		return def
	}
	v, err := strconv.Atoi(raw)
	if err != nil || v < 0 {
		slog.Warn("invalid transfer limit, keeping default",
			"var", name, "value", raw, "default", def)
		return def
	}
	return v
}

// LogSummary records the effective limits at startup. Without it there is no
// way to tell a default apart from a deliberate configuration after the fact.
func LogSummary(l Limits) {
	slog.Info("transfer limits in effect",
		"single_file", describe(l.MaxSingleFileBytes),
		"total_transfer", describe(l.MaxTotalTransferBytes),
		"clipboard_image", describe(l.MaxClipboardImageBytes),
		"clipboard_text", describe(l.MaxClipboardTextBytes),
		"items_per_offer", describeCount(l.MaxItemsPerOffer),
		"concurrent_transfers", describeCount(l.MaxConcurrentTransfers),
	)
}

// CheckOffer validates an offer against the limits and returns nil when it
// passes.
//
// The limits fall into two tiers that must not be mixed:
//
//   - Universal: item count and total bytes apply to every data_type. They
//     protect relay capacity, which no content type is exempt from.
//   - Content: mutually exclusive by data_type. FILES is bounded by
//     MaxSingleFileBytes, IMAGE by MaxClipboardImageBytes, TEXT by
//     MaxClipboardTextBytes.
//
// The exclusivity is the point. Applying the file limit to a clipboard image
// as well would let MaxSingleFileBytes silently override an image limit the
// operator deliberately raised, which turns MaxClipboardImageBytes into
// another knob that looks configured but does nothing — and it would report
// "文件太大" for something the user pasted rather than picked, contradicting
// the requirement's own wording. An oversized image is still caught by the
// universal total, which reports accurately.
//
// Check order is fixed rather than map-driven so the reported code is
// deterministic when several limits are exceeded at once: count, then the
// content dimension, then the total. Content precedes total because "one file
// is over 128 MB" tells the user exactly which file to drop, whereas the
// aggregate leaves them removing items one at a time.
func CheckOffer(l Limits, offer *protocol.TransferOfferPayload) *Violation {
	// Negative sizes are rejected outright. They are not reachable from an
	// honest client, and left alone they would drag the computed total down
	// and slip a genuinely oversized transfer past the total check.
	for i := range offer.Items {
		if offer.Items[i].Size < 0 {
			return &Violation{
				Code:    CodeMalformedOffer,
				Message: "传输请求包含非法的文件大小，已拒绝",
			}
		}
	}
	if offer.TotalSize < 0 {
		return &Violation{
			Code:    CodeMalformedOffer,
			Message: "传输请求包含非法的总大小，已拒绝",
		}
	}

	if l.MaxItemsPerOffer > 0 && len(offer.Items) > l.MaxItemsPerOffer {
		return &Violation{
			Code: CodeTooManyItems,
			Message: fmt.Sprintf("一次最多传输 %d 个文件，本次选择了 %d 个，无法传输",
				l.MaxItemsPerOffer, len(offer.Items)),
		}
	}

	if v := checkContent(l, offer); v != nil {
		return v
	}

	// The declared total is not trusted over the items it claims to summarise:
	// a client that under-reports TotalSize would otherwise buy itself room it
	// has not been granted. This does not make the limit enforceable against a
	// tampered client (nothing here counts bytes on the wire) — it just stops
	// the cheapest way of getting it wrong.
	total := offer.TotalSize
	var sum int64
	for i := range offer.Items {
		sum += offer.Items[i].Size
	}
	if sum > total {
		total = sum
	}

	if l.MaxTotalTransferBytes > 0 && total > l.MaxTotalTransferBytes {
		return &Violation{
			Code: CodeTotalTooLarge,
			Message: fmt.Sprintf("单次传输总量最大 %s，本次为 %s，无法传输",
				humanBytes(l.MaxTotalTransferBytes), humanBytes(total)),
		}
	}

	return nil
}

func checkContent(l Limits, offer *protocol.TransferOfferPayload) *Violation {
	switch offer.DataType {
	case "IMAGE":
		return clipboardViolation(offer, l.MaxClipboardImageBytes, CodeImageTooLarge, "剪贴板图片")
	case "TEXT":
		return clipboardViolation(offer, l.MaxClipboardTextBytes, CodeTextTooLarge, "剪贴板文本")
	default:
		// FILES, and anything a future client sends that we do not recognise.
		// Falling back to the file rule keeps an unknown type bounded instead
		// of unbounded.
		if l.MaxSingleFileBytes <= 0 {
			return nil
		}
		for i := range offer.Items {
			if offer.Items[i].Size > l.MaxSingleFileBytes {
				return &Violation{
					Code: CodeFileTooLarge,
					Message: fmt.Sprintf("单个文件最大 %s，「%s」为 %s，无法传输",
						humanBytes(l.MaxSingleFileBytes),
						offer.Items[i].RelativePath,
						humanBytes(offer.Items[i].Size)),
				}
			}
		}
		return nil
	}
}

// clipboardViolation bounds a clipboard offer by both its declared total and
// each of its items. In practice a clipboard offer carries exactly one item,
// so the two agree; checking both costs nothing and avoids a gap if that ever
// stops being true.
func clipboardViolation(offer *protocol.TransferOfferPayload, limit int64, code, label string) *Violation {
	if limit <= 0 {
		return nil
	}
	largest := offer.TotalSize
	for i := range offer.Items {
		if offer.Items[i].Size > largest {
			largest = offer.Items[i].Size
		}
	}
	if largest > limit {
		return &Violation{
			Code: code,
			Message: fmt.Sprintf("%s最大 %s，本次为 %s，无法传输",
				label, humanBytes(limit), humanBytes(largest)),
		}
	}
	return nil
}

// ConcurrencyViolation renders the user-facing message for a refusal that
// RelayManager has already decided. The decision itself happens under the
// relay lock (AuthorizeSessionIfUnderLimit); this only formats it.
//
// It still returns nil when the limit is disabled or unreached, so it stays
// safe to call defensively.
// number of authorized-but-unfinished sessions.
func ConcurrencyViolation(l Limits, inFlight int) *Violation {
	if l.MaxConcurrentTransfers <= 0 || inFlight < l.MaxConcurrentTransfers {
		return nil
	}
	return &Violation{
		Code: CodeTooManyConcurrent,
		Message: fmt.Sprintf("最多同时进行 %d 个传输，当前已有 %d 个，请稍后重试",
			l.MaxConcurrentTransfers, inFlight),
	}
}

func humanBytes(n int64) string {
	const unit = 1024
	if n < unit {
		return fmt.Sprintf("%d B", n)
	}
	div, exp := int64(unit), 0
	for v := n / unit; v >= unit && exp < 3; v /= unit {
		div *= unit
		exp++
	}
	return fmt.Sprintf("%.1f %cB", float64(n)/float64(div), "KMGT"[exp])
}

func describe(n int64) string {
	if n <= 0 {
		return "unlimited"
	}
	return humanBytes(n)
}

func describeCount(n int) string {
	if n <= 0 {
		return "unlimited"
	}
	return strconv.Itoa(n)
}
