package auth

import (
	"strings"
	"testing"
)

// 删掉这条断言，「忘了填账号」的客户端会重新汇集到同一个空账号公共桶里，
// 而 registry 的复合键会把它们当成一个真实账号来分区。
func TestValidateAccountIDRejectsEmpty(t *testing.T) {
	if err := ValidateAccountID(""); err == nil {
		t.Fatal("空账号标识必须被拒绝")
	}
}

// 纯空格必须挂：客户端保存时会 trim，但服务端不得信任客户端 trim 过。
// 设置面板的原生 required 恰恰放行这种输入（见 SettingsModal 的注释）。
func TestValidateAccountIDRejectsWhitespaceOnly(t *testing.T) {
	for _, s := range []string{" ", "   ", "\t", "\n"} {
		if err := ValidateAccountID(s); err == nil {
			t.Fatalf("纯空白账号标识必须被拒绝：%q", s)
		}
	}
}

// 删掉这条，canonical string 的字段分隔符就能被客户端重排 —— 分隔符注入。
func TestValidateAccountIDRejectsNewlineInjection(t *testing.T) {
	for _, s := range []string{"a\nb", "a\rb", "acct\nUNIDROP_V1"} {
		if err := ValidateAccountID(s); err == nil {
			t.Fatalf("含换行的账号标识必须被拒绝：%q", s)
		}
	}
}

func TestValidateAccountIDRejectsOverLength(t *testing.T) {
	if err := ValidateAccountID(strings.Repeat("a", MaxIdentifierLen)); err != nil {
		t.Fatalf("恰好 %d 字节应当通过", MaxIdentifierLen)
	}
	if err := ValidateAccountID(strings.Repeat("a", MaxIdentifierLen+1)); err == nil {
		t.Fatalf("超过 %d 字节必须被拒绝", MaxIdentifierLen)
	}
}

// 非 ASCII 会因 NFC/NFD 两种字节形式肉眼相同而静默分成两个桶 ——
// 与空账号公共桶是同一个缺陷的镜像。
func TestValidateAccountIDRejectsNonASCII(t *testing.T) {
	for _, s := range []string{"团队", "café", "acct/other", "acct:1", "a b"} {
		if err := ValidateAccountID(s); err == nil {
			t.Fatalf("非法字符的账号标识必须被拒绝：%q", s)
		}
	}
}

// 这条钉死「新规则没有把已发布的默认值和文档示例打死」。
// 它红了就说明这次收紧会让现有安装在升级后连不上。
func TestValidateAccountIDAcceptsShippedValues(t *testing.T) {
	for _, s := range []string{
		"default_user",   // client 的 default_config
		"my_team_sync",   // USER_GUIDE 示例
		"user1", "alice", "acct", // 既有测试用值
		"a.b", "user@example.com", "A-1_2",
	} {
		if err := ValidateAccountID(s); err != nil {
			t.Fatalf("已发布或文档中的账号标识必须通过：%q (%v)", s, err)
		}
	}
}

// device_id 刻意不强制 UUID：测试套件用 devA 这类短名，
// 强制格式会锁死非 Tauri 客户端而毫无安全收益。
func TestValidateDeviceIDAcceptsUUIDAndShortNames(t *testing.T) {
	for _, s := range []string{
		"550e8400-e29b-41d4-a716-446655440000",
		"devA", "dev-1", "dev_1", "MacBookPro",
	} {
		if err := ValidateDeviceID(s); err != nil {
			t.Fatalf("合法设备标识被误拒：%q (%v)", s, err)
		}
	}
}

func TestValidateDeviceIDRejectsEmptyAndControlChars(t *testing.T) {
	for _, s := range []string{"", " ", "dev\n1", "dev\x00", "dev.1", "dev@1"} {
		if err := ValidateDeviceID(s); err == nil {
			t.Fatalf("非法设备标识必须被拒绝：%q", s)
		}
	}
}

// session_id 走与 device_id 相同的文法，UUIDv4 天然通过。
func TestValidateSessionIDAcceptsUUID(t *testing.T) {
	if err := ValidateSessionID("550e8400-e29b-41d4-a716-446655440000"); err != nil {
		t.Fatalf("UUID 形态的会话标识必须通过：%v", err)
	}
	if err := ValidateSessionID(strings.Repeat("x", MaxIdentifierLen+1)); err == nil {
		t.Fatal("超长会话标识必须被拒绝：它会被当成 map 键")
	}
}

// 校验跑在**未鉴权**的攻击者可控输入上（控制面读上限 512 KiB），
// 所以必须先查长度再扫字符集，且全程零分配。
// 有人把实现改成 regexp 或先 TrimSpace，这条会红。
func TestValidateIsAllocationFreeOnOversizedInput(t *testing.T) {
	huge := strings.Repeat("a", 512*1024)
	allocs := testing.AllocsPerRun(100, func() {
		_ = ValidateAccountID(huge)
		_ = ValidateDeviceID(huge)
		_ = ValidateSessionID(huge)
	})
	if allocs != 0 {
		t.Fatalf("校验必须零分配，实测每次 %v 次分配", allocs)
	}
}
