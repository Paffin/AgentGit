package review

import (
	"fmt"
	"regexp"
	"strings"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/glob"
)

// Secret-shaped test values are assembled at run time so this source file does
// not itself contain anything a secret scanner would flag.
func awsKey() string    { return "AKIA" + "QWERTYUIOPASDFGH" }
func ghToken() string   { return "ghp_" + strings.Repeat("aB3dE5gH7j", 3) + "K9mN1p" }
func stripeKey() string { return "sk_" + "live_" + "4eC39HqLyjWDarjtT1zdp7dc" }
func slackToken() string {
	return "xoxb" + "-123456789012-" + "AbCdEfGhIjKlMnOp"
}
func jwt() string {
	return "eyJ" + "hbGciOiJIUzI1NiJ9" + ".eyJ" + "zdWIiOiIxMjM0NTY3ODkwIn0" + ".dBjftJeZ4CVPmB92K27uhbUJU1p1r"
}
func pemHeader() string { return "-----BEGIN RSA " + "PRIVATE KEY-----" }

func settings() Settings {
	set := func(p ...string) glob.Set {
		s, err := glob.CompileAll(p)
		if err != nil {
			panic(err)
		}
		return s
	}
	return Settings{
		MaxFiles: 40, MaxLines: 1500, Secrets: true, Tests: true,
		Protected: set(".github/**", "**/.env", "**/.env.*", "**/*.pem", "SECURITY.md"),
		Generated: set("**/*.lock", "**/package-lock.json", "**/go.sum", "**/dist/**", "**/vendor/**"),
		Sources:   set("**/*.go", "**/*.rs", "**/*.py", "**/*.ts"),
	}
}

// patch builds a unified diff that adds the given lines at the start of a file.
func patch(lines ...string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "--- a/f\n+++ b/f\n@@ -0,0 +1,%d @@\n", len(lines))
	for _, l := range lines {
		b.WriteString("+" + l + "\n")
	}
	return b.String()
}

func file(p, status string, lines ...string) agwp.FileDiff {
	return agwp.FileDiff{Path: p, Status: status, Additions: len(lines), Patch: patch(lines...)}
}

func kinds(fs []Finding) string {
	var k []string
	for _, f := range fs {
		k = append(k, f.Kind)
	}
	return strings.Join(k, ",")
}

func TestSecretsAreFound(t *testing.T) {
	cases := map[string]string{
		"AWS access key id":     `aws_access_key_id = "` + awsKey() + `"`,
		"GitHub token":          `export GH="` + ghToken() + `"`,
		"Stripe key":            `stripe.api_key = "` + stripeKey() + `"`,
		"Slack token":           `SLACK=` + slackToken(),
		"JSON web token":        `Authorization: Bearer ` + jwt(),
		"private key block":     pemHeader(),
		"hard-coded credential": `db_password = "Zx9kLm3Qp7Rt1Vw5Yb2N"`,
	}
	for want, line := range cases {
		fs, _ := Analyze([]agwp.FileDiff{file("config/app.go", "added", "package config", line)}, settings())
		if kinds(fs) != KindSecret {
			t.Errorf("%s: findings = %s for %q", want, kinds(fs), line)
			continue
		}
		detail := strings.Join(fs[0].Details, "\n")
		if !strings.Contains(detail, want) || !strings.Contains(detail, "config/app.go:2") {
			t.Errorf("%s: detail does not name the rule and the line:\n%s", want, detail)
		}
		if fs[0].Severity != SevHigh {
			t.Errorf("%s: severity %s", want, fs[0].Severity)
		}
	}
}

func TestSecretsAreNeverQuotedInFull(t *testing.T) {
	fs, _ := Analyze([]agwp.FileDiff{file("a.go", "added", `key := "`+awsKey()+`"`, `db_password = "Zx9kLm3Qp7Rt1Vw5Yb2N"`)}, settings())
	out := strings.Join(fs[0].Details, "\n")
	for _, secret := range []string{awsKey(), "Zx9kLm3Qp7Rt1Vw5Yb2N"} {
		if strings.Contains(out, secret) {
			t.Fatalf("the finding repeats the secret %q:\n%s", secret, out)
		}
	}
	if !strings.Contains(out, "chars)") {
		t.Fatalf("expected a redacted hint with the length:\n%s", out)
	}
}

// Conservative means: ordinary code and obvious placeholders must not be flagged.
func TestNoFalsePositivesOnOrdinaryCode(t *testing.T) {
	clean := []string{
		`password := os.Getenv("DB_PASSWORD")`,
		`token = "${TOKEN}"`,
		`password: "{{ .Values.password }}"`,
		`secret = "changeme-changeme-changeme"`,
		`api_key = "your-api-key-goes-here-123"`,
		`key := "AKIAIOSFODNN7EXAMPLE"`,
		`commit = "9fceb02d0ae598e95dc970b74767f19372d61af8"`,
		`token = "123e4567-e89b-12d3-a456-426614174000"`,
		`const tokenKind = "bearer-token-kind-identifier"`,
		`func tokenize(input string) []string { return strings.Fields(input) }`,
		`passwordStrength := score(len(pw), classes(pw))`,
		`secretary = "Marianne Fortescue-Smith the Third"`,
		`integrity "sha512-Vh2k0pQ8xZ1mB4nC7dE9fG3hJ5kL6mN8oP0qR2sT4uV6wX8yZ0aB2cD4eF6gH8iJ"`,
		`password = ""`,
		`password = "short"`,
		`// token: refresh before it expires (see docs)`,
		`let api_key = std::env::var("API_KEY")?;`,
		`if auth_token == nil { return ErrUnauthenticated }`,
		`fmt.Sprintf("password=%s", pw)`,
		`secret_name = "projects/1234567890/secrets/db-password/versions/latest"`,
		`![logo](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAHnOcEQAAAABJRU5ErkJggg==)`,
		`mock_token = "test-token-0123456789abcdef"`,
		`password = "password-123456789012345"`,
	}
	for _, line := range clean {
		fs, _ := Analyze([]agwp.FileDiff{file("pkg/x.go", "modified", line)}, settings())
		for _, f := range fs {
			if f.Kind == KindSecret {
				t.Errorf("false positive on %q: %v", line, f.Details)
			}
		}
	}
}

func TestSecretAllowances(t *testing.T) {
	line := `aws_access_key_id = "` + awsKey() + `"`
	s := settings()
	if fs, _ := Analyze([]agwp.FileDiff{file("a.go", "added", line+" // ag:allow-secret")}, s); len(fs) != 0 {
		t.Fatalf("inline allow ignored: %v", fs)
	}
	s.Allow = []*regexp.Regexp{regexp.MustCompile(`aws_access_key_id`)}
	if fs, _ := Analyze([]agwp.FileDiff{file("a.go", "added", line)}, s); len(fs) != 0 {
		t.Fatalf("configured allow pattern ignored: %v", fs)
	}
	s = settings()
	s.Secrets = false
	if fs, _ := Analyze([]agwp.FileDiff{file("a.go", "added", line)}, s); len(fs) != 0 {
		t.Fatalf("disabled check still ran: %v", fs)
	}
}

func TestOnlyAddedLinesAreScanned(t *testing.T) {
	p := "--- a/f\n+++ b/f\n@@ -1,2 +1,1 @@\n-key = \"" + awsKey() + "\"\n context line\n"
	fs, _ := Analyze([]agwp.FileDiff{{Path: "a.go", Status: "modified", Deletions: 1, Patch: p}}, settings())
	if len(fs) != 0 {
		t.Fatalf("removing a secret is not adding one: %v", fs)
	}
}

func TestBinaryAndGeneratedFilesAreNotScannedForSecrets(t *testing.T) {
	fs, _ := Analyze([]agwp.FileDiff{
		{Path: "logo.png", Status: "added", Binary: true, Patch: patch(`key = "` + awsKey() + `"`)},
		file("yarn.lock", "modified", `key = "`+awsKey()+`"`),
	}, settings())
	for _, f := range fs {
		if f.Kind == KindSecret {
			t.Fatalf("scanned a file that should be skipped: %v", f)
		}
	}
}

func TestAddedLineNumbersFollowTheHunkHeaders(t *testing.T) {
	p := "--- a/f\n+++ b/f\n@@ -10,3 +20,4 @@\n ctx\n+added one\n ctx2\n+added two\n@@ -40,1 +50,2 @@\n-old\n+new\n+newer\n"
	got := addedLines(p)
	want := []addedLine{{21, "added one"}, {23, "added two"}, {50, "new"}, {51, "newer"}}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Fatalf("addedLines = %v, want %v", got, want)
	}
}

func TestProtectedPaths(t *testing.T) {
	fs, _ := Analyze([]agwp.FileDiff{
		file(".github/workflows/ci.yml", "modified", "name: ci"),
		file("deploy/.env.production", "added", "A=1"),
		file("docs/readme.md", "modified", "hello"),
	}, settings())
	if kinds(fs) != KindProtected {
		t.Fatalf("findings = %s", kinds(fs))
	}
	d := strings.Join(fs[0].Details, "\n")
	if !strings.Contains(d, ".github/workflows/ci.yml") || !strings.Contains(d, "deploy/.env.production") || strings.Contains(d, "docs/readme.md") {
		t.Fatalf("details:\n%s", d)
	}
}

func TestOversizedChange(t *testing.T) {
	var files []agwp.FileDiff
	for i := 0; i < 41; i++ {
		files = append(files, agwp.FileDiff{Path: fmt.Sprintf("docs/f%d.md", i), Status: "added", Additions: 1, Patch: patch("x")})
	}
	fs, sum := Analyze(files, settings())
	if kinds(fs) != KindOversized || sum.Files != 41 || sum.Added != 41 {
		t.Fatalf("findings = %s summary = %+v", kinds(fs), sum)
	}
	// by lines
	fs, _ = Analyze([]agwp.FileDiff{{Path: "docs/big.md", Status: "added", Additions: 1501, Patch: patch("x")}}, settings())
	if kinds(fs) != KindOversized {
		t.Fatalf("findings = %s", kinds(fs))
	}
	// at the limit is fine; generated files do not count towards it
	files = nil
	for i := 0; i < 40; i++ {
		files = append(files, agwp.FileDiff{Path: fmt.Sprintf("docs/f%d.md", i), Status: "added", Additions: 1, Patch: patch("x")})
	}
	files = append(files, agwp.FileDiff{Path: "package-lock.json", Status: "modified", Additions: 3000, Patch: patch("x")})
	fs, _ = Analyze(files, settings())
	if kinds(fs) == KindOversized || strings.Contains(kinds(fs), KindOversized) {
		t.Fatalf("limit is inclusive and generated churn is separate: %s", kinds(fs))
	}
}

func TestMissingTests(t *testing.T) {
	many := make([]string, 12)
	for i := range many {
		many[i] = fmt.Sprintf("line%d := %d", i, i)
	}
	srcOnly := []agwp.FileDiff{file("pkg/handler.go", "modified", many...)}
	if fs, _ := Analyze(srcOnly, settings()); kinds(fs) != KindTests {
		t.Fatalf("source change without tests: %s", kinds(fs))
	}
	cases := map[string][]agwp.FileDiff{
		"a Go test changed":      {file("pkg/handler.go", "modified", many...), file("pkg/handler_test.go", "modified", "t")},
		"a Python test changed":  {file("app/views.py", "modified", many...), file("tests/test_views.py", "added", "t")},
		"a TypeScript spec":      {file("src/a.ts", "modified", many...), file("src/a.spec.ts", "added", "t")},
		"a test directory":       {file("pkg/handler.go", "modified", many...), file("test/fixtures/data.json", "added", "{}")},
		"inline Rust tests":      {file("src/lib.rs", "modified", append([]string{"#[cfg(test)]", "mod tests {"}, many...)...)},
		"a tiny source change":   {file("pkg/handler.go", "modified", "a", "b")},
		"documentation only":     {file("docs/guide.md", "modified", many...)},
		"configuration only":     {file("deploy/app.yaml", "modified", many...)},
		"a deleted source file":  {{Path: "pkg/old.go", Status: "deleted", Deletions: 50, Patch: "--- a/pkg/old.go\n+++ /dev/null\n"}},
		"generated sources only": {file("dist/bundle.ts", "modified", many...)},
	}
	for name, files := range cases {
		for _, f := range func() []Finding { fs, _ := Analyze(files, settings()); return fs }() {
			if f.Kind == KindTests {
				t.Errorf("%s: must not ask for tests", name)
			}
		}
	}
	s := settings()
	s.Tests = false
	if fs, _ := Analyze(srcOnly, s); len(fs) != 0 {
		t.Fatalf("disabled check still ran: %v", fs)
	}
}

func TestGeneratedAndBinaryChurn(t *testing.T) {
	if fs, _ := Analyze([]agwp.FileDiff{{Path: "go.sum", Status: "modified", Additions: 4, Deletions: 2, Patch: patch("x")}}, settings()); len(fs) != 0 {
		t.Fatalf("a small lockfile change is routine: %v", fs)
	}
	fs, _ := Analyze([]agwp.FileDiff{{Path: "go.sum", Status: "modified", Additions: 250, Patch: patch("x")}}, settings())
	if kinds(fs) != KindGenerated {
		t.Fatalf("big generated churn: %s", kinds(fs))
	}
	fs, _ = Analyze([]agwp.FileDiff{{Path: "assets/logo.png", Status: "added", Binary: true}}, settings())
	if kinds(fs) != KindGenerated || !strings.Contains(strings.Join(fs[0].Details, " "), "1 binary") {
		t.Fatalf("binary file: %v", fs)
	}
	var many []agwp.FileDiff
	for i := 0; i < 5; i++ {
		many = append(many, agwp.FileDiff{Path: fmt.Sprintf("dist/f%d.js", i), Status: "modified", Additions: 1, Patch: patch("x")})
	}
	if fs, _ := Analyze(many, settings()); kinds(fs) != KindGenerated {
		t.Fatalf("many generated files: %s", kinds(fs))
	}
}

func TestCleanChangesProduceNoFindings(t *testing.T) {
	clean := []agwp.FileDiff{
		file("pkg/handler.go", "modified", "func (h *Handler) Serve() error {", "\treturn nil", "}"),
		file("pkg/handler_test.go", "modified", "func TestServe(t *testing.T) {}"),
		file("docs/guide.md", "modified", "Some words."),
	}
	if fs, sum := Analyze(clean, settings()); len(fs) != 0 || sum.Files != 3 {
		t.Fatalf("findings = %v, summary = %+v", fs, sum)
	}
}

func TestFindingsAreOrderedBySeverityAndDeterministic(t *testing.T) {
	files := []agwp.FileDiff{
		{Path: "go.sum", Status: "modified", Additions: 300, Patch: patch("x")},
		file(".github/workflows/ci.yml", "modified", "run: echo"),
		file("config.go", "added", `key = "`+awsKey()+`"`),
	}
	first, _ := Analyze(files, settings())
	if kinds(first) != "secret,protected_path,generated_churn" {
		t.Fatalf("order = %s", kinds(first))
	}
	for i := 0; i < 20; i++ {
		again, _ := Analyze(files, settings())
		if fmt.Sprint(again) != fmt.Sprint(first) {
			t.Fatal("non-deterministic findings")
		}
	}
}

func TestEntropy(t *testing.T) {
	if entropy("aaaaaaaa") != 0 || entropy("") != 0 {
		t.Fatal("constant strings have no entropy")
	}
	if e := entropy("abcdefghijklmnop"); e < 3.9 || e > 4.1 {
		t.Fatalf("entropy = %v", e)
	}
	if !looksSecret("db_password", "Zx9kLm3Qp7Rt1Vw5Yb2N") || looksSecret("db_password", "aaaaaaaaaaaaaaaa1111") ||
		looksSecret("db_password", "this-is-just-a-long-name-without-digits") || looksSecret("session_token_name", "Zx9kLm3Qp7Rt1Vw5Yb2N") ||
		!isResourcePath("projects/123/secrets/db-password/versions/latest") || isResourcePath("wJalrXUtnFEM/K7MDENGbPxRfiCY") {
		t.Fatal("looksSecret thresholds")
	}
}
