package controller_test

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/unidrop/unidrop-server/internal/auth"
	"github.com/unidrop/unidrop-server/internal/limits"
	"github.com/unidrop/unidrop-server/internal/protocol"
)

// 握手层的身份校验。单元测试（auth/identity_test.go）盯的是文法本身，
// 这一组盯的是它确实接在握手里、走既有的 AUTH_RESPONSE 拒绝通道，
// 且错误码是客户端能分辨的那三个。

func TestEmptyAccountIDRejectedAtHandshake(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	_, resp := h.connectRaw(t, ctx, "", "dev1")
	if resp.Success {
		t.Fatal("空账号标识必须被握手拒绝")
	}
	if resp.ErrorCode != protocol.AuthErrInvalidAccountID {
		t.Fatalf("期望错误码 %s，实得 %s", protocol.AuthErrInvalidAccountID, resp.ErrorCode)
	}
	// 文案必须把规则本身告诉用户：只说「账号非法」他改不对。
	if !strings.Contains(resp.ErrorMessage, "1-64") {
		t.Fatalf("拒绝文案应当写明格式规则，实得：%s", resp.ErrorMessage)
	}
}

func TestWhitespaceAccountIDRejectedAtHandshake(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	_, resp := h.connectRaw(t, ctx, "   ", "dev1")
	if resp.Success {
		t.Fatal("纯空格账号标识必须被拒绝：设置面板的原生 required 放行它")
	}
	if resp.ErrorCode != protocol.AuthErrInvalidAccountID {
		t.Fatalf("期望错误码 %s，实得 %s", protocol.AuthErrInvalidAccountID, resp.ErrorCode)
	}
}

func TestNewlineAccountIDRejectedAtHandshake(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	_, resp := h.connectRaw(t, ctx, "acct\nUNIDROP_V1", "dev1")
	if resp.Success {
		t.Fatal("含换行的账号标识必须被拒绝：canonical string 以换行分隔字段")
	}
	if resp.ErrorCode != protocol.AuthErrInvalidAccountID {
		t.Fatalf("期望错误码 %s，实得 %s", protocol.AuthErrInvalidAccountID, resp.ErrorCode)
	}
}

func TestEmptyDeviceIDRejectedAtHandshake(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	_, resp := h.connectRaw(t, ctx, "acctA", "")
	if resp.Success {
		t.Fatal("空设备标识必须被握手拒绝")
	}
	if resp.ErrorCode != protocol.AuthErrInvalidDeviceID {
		t.Fatalf("期望错误码 %s，实得 %s", protocol.AuthErrInvalidDeviceID, resp.ErrorCode)
	}
}

func TestOverlongAccountIDRejectedAtHandshake(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	_, resp := h.connectRaw(t, ctx, strings.Repeat("a", auth.MaxIdentifierLen+1), "dev1")
	if resp.Success {
		t.Fatal("超长账号标识必须被握手拒绝")
	}
	if resp.ErrorCode != protocol.AuthErrInvalidAccountID {
		t.Fatalf("期望错误码 %s，实得 %s", protocol.AuthErrInvalidAccountID, resp.ErrorCode)
	}
}

// 正例：防止规则被收紧成拒绝一切。
func TestValidIdentifiersStillAuthenticate(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	for _, acct := range []string{"default_user", "my_team_sync", "user@example.com"} {
		_, resp := h.connectRaw(t, ctx, acct, "550e8400-e29b-41d4-a716-446655440000")
		if !resp.Success {
			t.Fatalf("合法账号标识 %q 必须通过握手，实得：%s", acct, resp.ErrorMessage)
		}
	}
}

// 钉死「格式校验排在验签之前」这个顺序决策。
//
// VerifyWithSalt 会在签名通过后烧掉 nonce。若格式校验排在它之后，
// 用同一个 nonce 改正账号再试就会撞上重放判定，错误从「账号格式非法」
// 漂成「replay detected」，第三方客户端据协议契约复用 nonce 时永远改不对。
func TestRejectedIdentityDoesNotBurnNonce(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h := newLimitsHarness(t, limits.Limits{})

	nonce := uuid.NewString()

	bad := h.connectRawWithNonce(t, ctx, "", "dev1", nonce)
	if bad.Success || bad.ErrorCode != protocol.AuthErrInvalidAccountID {
		t.Fatalf("第一次应当因账号格式被拒，实得 success=%v code=%s", bad.Success, bad.ErrorCode)
	}

	good := h.connectRawWithNonce(t, ctx, "acctA", "dev1", nonce)
	if !good.Success {
		t.Fatalf("改正账号后用同一个 nonce 重试必须成功（格式拒绝不得烧 nonce），实得：%s",
			good.ErrorMessage)
	}
}
