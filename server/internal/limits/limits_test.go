package limits

import (
	"testing"

	"github.com/unidrop/unidrop-server/internal/protocol"
)

func filesOffer(sizes ...int64) *protocol.TransferOfferPayload {
	offer := &protocol.TransferOfferPayload{DataType: "FILES"}
	for i, s := range sizes {
		offer.Items = append(offer.Items, protocol.TransferItemPayload{
			ItemIndex:    uint32(i),
			RelativePath: "f.bin",
			Size:         s,
		})
		offer.TotalSize += s
	}
	offer.TotalItems = len(offer.Items)
	return offer
}

func clipOffer(dataType string, size int64) *protocol.TransferOfferPayload {
	return &protocol.TransferOfferPayload{
		DataType:   dataType,
		TotalSize:  size,
		TotalItems: 1,
		Items: []protocol.TransferItemPayload{
			{ItemIndex: 0, RelativePath: "clipboard", Size: size},
		},
	}
}

func defaults() Limits {
	return Limits{
		MaxSingleFileBytes:     DefaultMaxSingleFileBytes,
		MaxTotalTransferBytes:  DefaultMaxTotalTransferBytes,
		MaxClipboardImageBytes: DefaultMaxClipboardImageBytes,
		MaxClipboardTextBytes:  DefaultMaxClipboardTextBytes,
		MaxItemsPerOffer:       DefaultMaxItemsPerOffer,
		MaxConcurrentTransfers: DefaultMaxConcurrentTransfers,
	}
}

// ---------- 边界：恰好等于上限必须通过 ----------

func TestExactlyAtLimitPasses(t *testing.T) {
	l := defaults()

	if v := CheckOffer(l, filesOffer(DefaultMaxSingleFileBytes)); v != nil {
		t.Fatalf("单文件恰好等于上限应通过，却被拒：%v", v)
	}
	if v := CheckOffer(l, clipOffer("IMAGE", DefaultMaxClipboardImageBytes)); v != nil {
		t.Fatalf("图片恰好等于上限应通过，却被拒：%v", v)
	}
	if v := CheckOffer(l, clipOffer("TEXT", DefaultMaxClipboardTextBytes)); v != nil {
		t.Fatalf("文本恰好等于上限应通过，却被拒：%v", v)
	}

	// 条目数恰好等于上限
	sizes := make([]int64, DefaultMaxItemsPerOffer)
	for i := range sizes {
		sizes[i] = 1
	}
	if v := CheckOffer(l, filesOffer(sizes...)); v != nil {
		t.Fatalf("条目数恰好等于上限应通过，却被拒：%v", v)
	}
}

func TestOneOverLimitRejected(t *testing.T) {
	l := defaults()

	if v := CheckOffer(l, filesOffer(DefaultMaxSingleFileBytes+1)); v == nil {
		t.Fatal("单文件超上限 1 字节必须被拒")
	} else if v.Code != CodeFileTooLarge {
		t.Fatalf("错误码应为 %s，实际 %s", CodeFileTooLarge, v.Code)
	}

	if v := CheckOffer(l, clipOffer("IMAGE", DefaultMaxClipboardImageBytes+1)); v == nil {
		t.Fatal("图片超上限必须被拒")
	} else if v.Code != CodeImageTooLarge {
		t.Fatalf("错误码应为 %s，实际 %s", CodeImageTooLarge, v.Code)
	}

	if v := CheckOffer(l, clipOffer("TEXT", DefaultMaxClipboardTextBytes+1)); v == nil {
		t.Fatal("文本超上限必须被拒")
	} else if v.Code != CodeTextTooLarge {
		t.Fatalf("错误码应为 %s，实际 %s", CodeTextTooLarge, v.Code)
	}

	sizes := make([]int64, DefaultMaxItemsPerOffer+1)
	for i := range sizes {
		sizes[i] = 1
	}
	if v := CheckOffer(l, filesOffer(sizes...)); v == nil {
		t.Fatal("条目数超上限必须被拒")
	} else if v.Code != CodeTooManyItems {
		t.Fatalf("错误码应为 %s，实际 %s", CodeTooManyItems, v.Code)
	}
}

// ---------- 0 = 不限制 ----------

func TestZeroMeansUnlimited(t *testing.T) {
	l := Limits{} // 全 0

	huge := int64(1) << 40 // 1 TiB
	if v := CheckOffer(l, filesOffer(huge)); v != nil {
		t.Fatalf("单文件上限为 0 时任意大小都应通过，却被拒：%v", v)
	}
	if v := CheckOffer(l, clipOffer("IMAGE", huge)); v != nil {
		t.Fatalf("图片上限为 0 时任意大小都应通过，却被拒：%v", v)
	}

	sizes := make([]int64, 5000)
	for i := range sizes {
		sizes[i] = 1
	}
	if v := CheckOffer(l, filesOffer(sizes...)); v != nil {
		t.Fatalf("条目数上限为 0 时任意条数都应通过，却被拒：%v", v)
	}

	if v := ConcurrencyViolation(l, 9999); v != nil {
		t.Fatalf("并发上限为 0 时不应拒绝：%v", v)
	}
}

// ---------- 内容维度按 data_type 互斥 ----------

// 这条守护的是计划 §4.4 的分层裁决（AGY-02）：图片走图片上限，**不再**
// 叠加单文件上限。若有人把两者改成「同时适用取更严」，运营调高的图片上限
// 会被单文件上限静默压制——那正是本轮要消灭的「配了不生效」。
func TestClipboardImageNotBoundBySingleFileLimit(t *testing.T) {
	l := defaults()
	l.MaxClipboardImageBytes = 512 * 1024 * 1024 // 运营特意调高到 512 MB
	l.MaxTotalTransferBytes = 0                  // 单独考察内容维度
	// 单文件上限保持 128 MB

	offer := clipOffer("IMAGE", 200*1024*1024) // 200 MB，高于单文件上限
	if v := CheckOffer(l, offer); v != nil {
		t.Fatalf("图片只应受图片上限约束，不应被单文件上限拦下：%v", v)
	}
}

// 反向：文件仍然受单文件上限约束，不会因为图片上限调高而放行。
func TestFilesStillBoundBySingleFileLimit(t *testing.T) {
	l := defaults()
	l.MaxClipboardImageBytes = 512 * 1024 * 1024
	l.MaxTotalTransferBytes = 0

	if v := CheckOffer(l, filesOffer(200*1024*1024)); v == nil {
		t.Fatal("FILES 必须仍受单文件上限约束")
	}
}

// 超大图片仍被「通用维度」的总量上限兜住，且报的是准确的那条错误码。
func TestOversizedImageStillCaughtByTotalLimit(t *testing.T) {
	l := defaults()
	l.MaxClipboardImageBytes = 512 * 1024 * 1024

	v := CheckOffer(l, clipOffer("IMAGE", 300*1024*1024)) // > 256 MB 总量
	if v == nil {
		t.Fatal("超过总量上限的图片必须被拒")
	}
	if v.Code != CodeTotalTooLarge {
		t.Fatalf("应报总量超限而非图片超限，实际 %s", v.Code)
	}
}

// ---------- 多项同时超限时错误码确定 ----------

func TestViolationOrderIsDeterministic(t *testing.T) {
	l := defaults()

	// 条目数、单文件、总量三者同时超限
	sizes := make([]int64, DefaultMaxItemsPerOffer+1)
	for i := range sizes {
		sizes[i] = DefaultMaxSingleFileBytes + 1
	}
	offer := filesOffer(sizes...)

	for i := 0; i < 50; i++ {
		v := CheckOffer(l, offer)
		if v == nil || v.Code != CodeTooManyItems {
			t.Fatalf("第 %d 次：条目数应优先报出，实际 %v", i, v)
		}
	}

	// 单文件与总量同时超限时，报更可操作的单文件
	v := CheckOffer(l, filesOffer(DefaultMaxSingleFileBytes+1, DefaultMaxTotalTransferBytes))
	if v == nil || v.Code != CodeFileTooLarge {
		t.Fatalf("单文件应优先于总量报出，实际 %v", v)
	}
}

// ---------- 文案必须带具体数字 ----------

// 计划 §4.1.1：条目数与总量是最常触发的两条，只说「太多」「太大」用户
// 无从下手。这条测试把「文案里必须出现上限数字」钉死。
func TestMessagesCarryActionableNumbers(t *testing.T) {
	l := defaults()

	sizes := make([]int64, DefaultMaxItemsPerOffer+1)
	for i := range sizes {
		sizes[i] = 1
	}
	v := CheckOffer(l, filesOffer(sizes...))
	if v == nil || !contains(v.Message, "64") {
		t.Fatalf("条目数超限文案必须写明上限 64，实际：%q", msgOf(v))
	}

	v = CheckOffer(l, filesOffer(DefaultMaxSingleFileBytes+1))
	if v == nil || !contains(v.Message, "128.0 MB") {
		t.Fatalf("单文件超限文案必须写明上限，实际：%q", msgOf(v))
	}
}

// ---------- 非法大小 ----------

func TestNegativeSizesRejected(t *testing.T) {
	l := defaults()

	// 负数会把求和拉低，从而把真正超限的传输混过总量检查。
	offer := filesOffer(DefaultMaxTotalTransferBytes, -DefaultMaxTotalTransferBytes)
	v := CheckOffer(l, offer)
	if v == nil || v.Code != CodeMalformedOffer {
		t.Fatalf("含负数大小的 offer 必须被拒为畸形，实际 %v", v)
	}
}

// 自报总量小于各条目之和时，以求和为准。
func TestUnderreportedTotalDoesNotBuyHeadroom(t *testing.T) {
	l := defaults()

	offer := filesOffer(100*1024*1024, 100*1024*1024, 100*1024*1024) // 实为 300 MB
	offer.TotalSize = 1                                              // 自报 1 字节

	v := CheckOffer(l, offer)
	if v == nil || v.Code != CodeTotalTooLarge {
		t.Fatalf("自报总量偏小不应换来额度，实际 %v", v)
	}
}

// ---------- env 三态解析 ----------

func TestEnvUnsetTakesDefault(t *testing.T) {
	l := FromEnv()
	if l.MaxSingleFileBytes != DefaultMaxSingleFileBytes {
		t.Fatalf("未设置时应取默认值 %d，实际 %d", DefaultMaxSingleFileBytes, l.MaxSingleFileBytes)
	}
	if l.MaxItemsPerOffer != DefaultMaxItemsPerOffer {
		t.Fatalf("未设置时应取默认值 %d，实际 %d", DefaultMaxItemsPerOffer, l.MaxItemsPerOffer)
	}
}

// 这条是本包最重要的一条。strconv 失败返回 0，而 0 在这里表示「不限制」，
// 所以一个写错的变量值（拼错、带单位、负数）若被直接采用，会**静默解除限制**，
// 而运营以为自己刚配好了上限。三态规则要求这种情况退回默认值。
func TestInvalidEnvFallsBackToDefaultNotZero(t *testing.T) {
	cases := []string{"2GB", "abc", "-1", "12.5", " 128", ""}

	for _, raw := range cases {
		t.Run("raw="+raw, func(t *testing.T) {
			t.Setenv(EnvMaxSingleFileBytes, raw)
			t.Setenv(EnvMaxItemsPerOffer, raw)

			l := FromEnv()
			if l.MaxSingleFileBytes != DefaultMaxSingleFileBytes {
				t.Fatalf("非法值 %q 必须退回默认值 %d，实际 %d —— 若为 0 则限制被静默解除",
					raw, DefaultMaxSingleFileBytes, l.MaxSingleFileBytes)
			}
			if l.MaxItemsPerOffer != DefaultMaxItemsPerOffer {
				t.Fatalf("非法值 %q 必须退回默认值 %d，实际 %d",
					raw, DefaultMaxItemsPerOffer, l.MaxItemsPerOffer)
			}
		})
	}
}

// 「显式设 0」与「未设置」必须可区分：前者是运营明确要关掉该项，
// 后者是没表态。两者若混为一谈，就无法关掉任何一项限制。
func TestExplicitZeroIsDistinguishableFromUnset(t *testing.T) {
	t.Setenv(EnvMaxSingleFileBytes, "0")
	t.Setenv(EnvMaxItemsPerOffer, "0")

	l := FromEnv()
	if l.MaxSingleFileBytes != 0 {
		t.Fatalf("显式设 0 应得到 0（不限制），实际 %d", l.MaxSingleFileBytes)
	}
	if l.MaxItemsPerOffer != 0 {
		t.Fatalf("显式设 0 应得到 0（不限制），实际 %d", l.MaxItemsPerOffer)
	}

	// 而未设置的那几项仍是默认值，证明两种状态确实分得开
	if l.MaxClipboardImageBytes != DefaultMaxClipboardImageBytes {
		t.Fatalf("未设置项应保持默认值，实际 %d", l.MaxClipboardImageBytes)
	}
}

func TestValidEnvIsTakenAsIs(t *testing.T) {
	t.Setenv(EnvMaxTotalTransferBytes, "1048576")
	t.Setenv(EnvMaxConcurrentTransfers, "3")

	l := FromEnv()
	if l.MaxTotalTransferBytes != 1048576 {
		t.Fatalf("合法值应原样采用，实际 %d", l.MaxTotalTransferBytes)
	}
	if l.MaxConcurrentTransfers != 3 {
		t.Fatalf("合法值应原样采用，实际 %d", l.MaxConcurrentTransfers)
	}
}

// 字节字段必须能表达超过 2 GiB 的值。用 strconv.Atoi 时在 32 位构建上会溢出。
func TestByteLimitsExceedingTwoGiB(t *testing.T) {
	t.Setenv(EnvMaxTotalTransferBytes, "8589934592") // 8 GiB

	l := FromEnv()
	if l.MaxTotalTransferBytes != 8589934592 {
		t.Fatalf("8 GiB 应被完整解析，实际 %d", l.MaxTotalTransferBytes)
	}
}

// ---------- 并发 ----------

func TestConcurrencyViolation(t *testing.T) {
	l := defaults() // 上限 8

	if v := ConcurrencyViolation(l, 7); v != nil {
		t.Fatalf("低于上限不应拒绝：%v", v)
	}
	if v := ConcurrencyViolation(l, 8); v == nil {
		t.Fatal("已达上限时新会话必须被拒")
	} else if v.Code != CodeTooManyConcurrent {
		t.Fatalf("错误码应为 %s，实际 %s", CodeTooManyConcurrent, v.Code)
	}
}

func contains(s, sub string) bool {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}

func msgOf(v *Violation) string {
	if v == nil {
		return "<nil>"
	}
	return v.Message
}
