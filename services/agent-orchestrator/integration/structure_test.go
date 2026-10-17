package integration

import (
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"path/filepath"
	"sort"
	"strings"
	"testing"
)

// The orchestrator proposes and schedules; it never lands changes, changes
// policy, moves a stream or administers the server. The typed client has the
// operations tests need to play administrators (internal/agwp/admin.go); this
// test fails if any other non-test code of the service calls them, or posts a
// review that is an approval or a request for changes.
var forbiddenCalls = map[string]string{
	"Integrate":     "lands a proposal",
	"SetPolicy":     "changes the stream policy",
	"GrantRole":     "grants roles",
	"CreateRepo":    "creates repositories",
	"CreateTask":    "creates tasks (people and other systems file them)",
	"CloseProposal": "closes proposals",
}

var forbiddenSelectors = map[string]string{
	"DecisionApprove": "an automated approval",
	"DecisionChanges": "an automated request for changes",
}

// violations returns "file:line: reason" for every forbidden use in src.
func violations(fset *token.FileSet, file *ast.File) []string {
	var out []string
	ast.Inspect(file, func(n ast.Node) bool {
		switch x := n.(type) {
		case *ast.CallExpr:
			if sel, ok := x.Fun.(*ast.SelectorExpr); ok {
				if why, bad := forbiddenCalls[sel.Sel.Name]; bad {
					out = append(out, fset.Position(x.Pos()).String()+": call to "+sel.Sel.Name+" ("+why+")")
				}
			}
		case *ast.SelectorExpr:
			if why, bad := forbiddenSelectors[x.Sel.Name]; bad {
				out = append(out, fset.Position(x.Pos()).String()+": use of "+x.Sel.Name+" ("+why+")")
			}
		}
		return true
	})
	return out
}

func TestOrchestratorNeverIntegrates(t *testing.T) {
	root := ".." // the module root
	fset := token.NewFileSet()
	checked := 0
	var found []string
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(root, path)
		rel = filepath.ToSlash(rel)
		if d.IsDir() {
			switch rel {
			case ".git", "internal/agwp", "internal/testutil", "integration":
				// the client library holds the operations on purpose; the fake server and the
				// integration tests play the humans and the server
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(path, ".go") || strings.HasSuffix(path, "_test.go") {
			return nil
		}
		f, err := parser.ParseFile(fset, path, nil, 0)
		if err != nil {
			return err
		}
		checked++
		found = append(found, violations(fset, f)...)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if checked < 30 {
		t.Fatalf("only %d source files were checked: the walk is broken", checked)
	}
	sort.Strings(found)
	for _, v := range found {
		t.Errorf("forbidden: %s", v)
	}
}

// The detector itself must see what it is meant to see, and the client library
// must really contain the operations (otherwise the test above proves nothing).
func TestTheStructuralCheckDetectsForbiddenUse(t *testing.T) {
	src := `package x
func f(c client) {
	c.Integrate(ctx, "r", "p", nil)
	_ = c.SetPolicy
	c.ReviewProposal(ctx, "r", "p", agwp.DecisionApprove, "")
	c.GrantRole(ctx, "a", "b", "c")
}`
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, "x.go", src, 0)
	if err != nil {
		t.Fatal(err)
	}
	got := violations(fset, f)
	if len(got) != 3 { // the Integrate call, the approval, the GrantRole call; a bare method value is not a call
		t.Fatalf("violations = %v", got)
	}
	pkg, err := parser.ParseDir(token.NewFileSet(), "../internal/agwp", nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	declared := map[string]bool{}
	for _, p := range pkg {
		for _, file := range p.Files {
			for _, d := range file.Decls {
				if fn, ok := d.(*ast.FuncDecl); ok && fn.Recv != nil {
					declared[fn.Name.Name] = true
				}
			}
		}
	}
	for name := range forbiddenCalls {
		if !declared[name] {
			t.Errorf("internal/agwp no longer declares %s: update the structural check", name)
		}
	}
}
