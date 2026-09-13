package controller_test

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/unidrop/unidrop-server/internal/controller"
	"github.com/unidrop/unidrop-server/internal/registry"
	"github.com/unidrop/unidrop-server/internal/relay"
)

func newMetricsFixture() (*registry.DeviceRegistry, *relay.RelayManager) {
	reg := registry.NewDeviceRegistry()
	reg.Register(registry.NewDeviceSession("acctA", "dev1", "h", "macos", "1", "127.0.0.1", nil))
	reg.Register(registry.NewDeviceSession("acctA", "dev2", "h", "macos", "1", "127.0.0.1", nil))
	reg.Register(registry.NewDeviceSession("acctB", "dev1", "h", "macos", "1", "127.0.0.1", nil))
	return reg, relay.NewRelayManager(0)
}

func scrape(t *testing.T, reg *registry.DeviceRegistry, rm *relay.RelayManager) string {
	t.Helper()
	rec := httptest.NewRecorder()
	controller.MetricsHandler(reg, rm)(rec, httptest.NewRequest("GET", "/metrics", nil))
	return rec.Body.String()
}

// 基数闸门的唯一自动化守卫。
//
// 它防的是「有人照 DESIGN.md 原稿去加 account_id label」——
// 那份原稿曾规定 unidrop_connected_devices{account_id, os_type}，
// 而 account_id 是未鉴权、客户端自报的任意字符串：
// 一个客户端循环握手就能铸出无限多个值，每个都在 exporter 里留一条常驻时序。
func TestMetricsHasNoAccountIDLabel(t *testing.T) {
	reg, rm := newMetricsFixture()
	body := scrape(t, reg, rm)

	if strings.Contains(body, "account_id=") {
		t.Fatalf("/metrics 不得出现 account_id label：它的基数无界。实际输出：\n%s", body)
	}
	// 账号名本身也不得出现在任何位置
	for _, acct := range []string{"acctA", "acctB"} {
		if strings.Contains(body, acct) {
			t.Fatalf("/metrics 不得泄露账号名 %q", acct)
		}
	}
}

func TestMetricsExposesAccountAggregates(t *testing.T) {
	reg, rm := newMetricsFixture()
	body := scrape(t, reg, rm)

	for _, want := range []string{
		"unidrop_online_devices 3",
		"unidrop_online_accounts 2",
		"unidrop_max_devices_per_account 2",
		"unidrop_max_pipes_per_account 0",
	} {
		if !strings.Contains(body, want) {
			t.Fatalf("/metrics 缺少或数值不符：%q\n实际输出：\n%s", want, body)
		}
	}
}

// reason 是代码里定义的闭集枚举，基数由构造保证有界 ——
// 这与 account_id 有本质区别，不能被当成「既然能加 label 那 account_id 也行」的先例。
func TestAuthRejectedCounterHasBoundedReasonSet(t *testing.T) {
	reg, rm := newMetricsFixture()
	body := scrape(t, reg, rm)

	for _, reason := range []string{"invalid_account_id", "invalid_device_id", "unauthorized"} {
		if !strings.Contains(body, `unidrop_auth_rejected_total{reason="`+reason+`"}`) {
			t.Fatalf("缺少拒绝计数分项 %q", reason)
		}
	}

	// 只应出现这三个 reason，不多不少
	if got := strings.Count(body, "unidrop_auth_rejected_total{"); got != 3 {
		t.Fatalf("拒绝计数应恰好三个分项，实得 %d", got)
	}
}

// /healthz 是未鉴权端点，列出账号名等于把全部账号交给任何能 GET 的人。
func TestHealthzDoesNotLeakAccountIDs(t *testing.T) {
	reg, rm := newMetricsFixture()

	rec := httptest.NewRecorder()
	controller.HealthHandler(reg, rm)(rec, httptest.NewRequest("GET", "/healthz", nil))
	body := rec.Body.String()

	for _, acct := range []string{"acctA", "acctB"} {
		if strings.Contains(body, acct) {
			t.Fatalf("/healthz 不得泄露账号名 %q：它没有鉴权", acct)
		}
	}

	var status map[string]any
	if err := json.Unmarshal([]byte(body), &status); err != nil {
		t.Fatalf("/healthz 应返回合法 JSON：%v", err)
	}
	for _, key := range []string{"online_accounts", "max_devices_per_account", "max_pipes_per_account"} {
		if _, ok := status[key]; !ok {
			t.Fatalf("/healthz 缺少聚合字段 %q", key)
		}
	}
	if got := status["online_accounts"]; got != float64(2) {
		t.Fatalf("online_accounts 应为 2，实得 %v", got)
	}
}
