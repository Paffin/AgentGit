# Homebrew formula TEMPLATE for AgentGit. Do not install from this file: @VERSION@ and the @SHA256_…@ checksums
# are filled in from a release's SHA256SUMS by scripts/render_packaging.py, which writes the real formula
# (the release workflow attaches it to every release as `agentgit.rb`). Put that file in a tap, e.g.
# github.com/Paffin/homebrew-agentgit (such a tap does not exist yet), as Formula/agentgit.rb.
class Agentgit < Formula
  desc "Agent-native source control: deterministic core, policy-bound autonomy, tamper-evident audit"
  homepage "https://github.com/Paffin/AgentGit"
  version "@VERSION@"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "@BASE_URL@/agentgit-@VERSION@-aarch64-apple-darwin.tar.gz"
      sha256 "@SHA256_aarch64-apple-darwin@"
    end
    on_intel do
      url "@BASE_URL@/agentgit-@VERSION@-x86_64-apple-darwin.tar.gz"
      sha256 "@SHA256_x86_64-apple-darwin@"
    end
  end

  on_linux do
    on_arm do
      url "@BASE_URL@/agentgit-@VERSION@-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256_aarch64-unknown-linux-gnu@"
    end
    on_intel do
      url "@BASE_URL@/agentgit-@VERSION@-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256_x86_64-unknown-linux-gnu@"
    end
  end

  def install
    bin.install "ag", "ag-server", "ag-worker", "ag-mcp"
    bin.install "orchestrator" if File.exist?("orchestrator")
    doc.install "README.md"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ag --version")
    assert_match version.to_s, shell_output("#{bin}/ag-server --version")
  end
end
