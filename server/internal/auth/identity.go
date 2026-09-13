package auth

import "errors"

// Identifier validation for the three client-reported strings that the server
// uses as map keys or as fields of the signed canonical string: account_id,
// device_id and session_id.
//
// These live in the auth package rather than in one of their own because they
// constrain the very same bytes that BuildCanonicalStringWithSalt formats. The
// most important rule below — no newlines — only makes sense next to the
// function whose field separator is "\n"; split across packages, the reason
// drifts away from the thing it protects and gets "simplified" out.
//
// Why newlines are forbidden: BuildCanonicalStringWithSalt joins six fields
// with "\n". An account_id containing a newline would let a client redraw the
// field boundaries of the string being signed — separator injection. With the
// current field set it is not exploitable (the timestamp must parse as a
// decimal integer and the nonce goes through the replay cache), but that is an
// argument about today's values, not about the shape of the construction. This
// is a whole class of bug and it should not survive on "happens not to be
// reachable".
//
// Why ASCII-only, i.e. why there are no Chinese account names: account_id is a
// matching key. "团队" in NFC and in NFD is the same string to a human eye and
// two different byte sequences to a map, so allowing it would silently split
// one workspace into two — the same defect as the empty-account bucket this
// validation exists to close, only mirrored. The identifiers also end up in
// logs and (as an aggregate) on an unauthenticated metrics endpoint.
//
// Why this is not configurable: a configurable identifier grammar is a knob
// nobody sets, a test matrix that doubles, and — since the only direction
// anyone would ever turn it is "looser" — a way to undo exactly what this file
// is for.
const MaxIdentifierLen = 64

var (
	ErrInvalidAccountID = errors.New("invalid account_id")
	ErrInvalidDeviceID  = errors.New("invalid device_id")
	ErrInvalidSessionID = errors.New("invalid session_id")
)

// ValidateAccountID checks that s is a well-formed account identifier.
//
// Allowed: 1..MaxIdentifierLen bytes of [A-Za-z0-9._@-]. The extra three
// characters over the device/session grammar are there because this one is
// typed by a human and mail-like or dotted names are the obvious things to
// reach for.
func ValidateAccountID(s string) error {
	if !validIdentifier(s, true) {
		return ErrInvalidAccountID
	}
	return nil
}

// ValidateDeviceID checks that s is a well-formed device identifier.
//
// Allowed: 1..MaxIdentifierLen bytes of [A-Za-z0-9_-]. Deliberately not
// restricted to UUIDs even though the shipped client generates one: the test
// suites use short names like devA, and pinning the format here would lock out
// any non-Tauri client for no security gain — the value is opaque to the
// server either way.
func ValidateDeviceID(s string) error {
	if !validIdentifier(s, false) {
		return ErrInvalidDeviceID
	}
	return nil
}

// ValidateSessionID checks that s is a well-formed transfer session identifier.
//
// Same grammar as device_id, which a UUIDv4 satisfies as-is. This one is not
// about partitioning — session IDs are minted per transfer and collisions are
// an anomaly, not a normal occurrence (see relay.RelayManager) — it only keeps
// a half-megabyte string from being used as a map key and keeps control
// characters out of the logs.
func ValidateSessionID(s string) error {
	if !validIdentifier(s, false) {
		return ErrInvalidSessionID
	}
	return nil
}

// validIdentifier reports whether s is non-empty, within the length cap, and
// built only from the permitted byte set.
//
// The length check runs before the character scan, and the whole thing is a
// hand-written byte loop with no allocation and no regexp. That is not
// micro-optimisation: callers run this on unauthenticated, attacker-controlled
// input — the control-plane read limit is 512 KiB and validation happens
// before signature verification — so the cost of rejecting garbage has to stay
// proportional to how quickly the garbage can be recognised. A regexp, or a
// strings.TrimSpace first, would allocate once per hostile frame.
func validIdentifier(s string, accountGrammar bool) bool {
	if len(s) == 0 || len(s) > MaxIdentifierLen {
		return false
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c >= 'A' && c <= 'Z':
		case c >= 'a' && c <= 'z':
		case c >= '0' && c <= '9':
		case c == '_' || c == '-':
		case accountGrammar && (c == '.' || c == '@'):
		default:
			return false
		}
	}
	return true
}
