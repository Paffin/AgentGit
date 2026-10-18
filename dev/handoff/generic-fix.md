# generic-fix handoff

State: the only red workflow on main was "OCI runtimes and cgroup v2" / docker (run 39135279298 at 27bbce1).
Cause: `the_memory_limit_is_set_in_the_cgroup_and_a_hog_is_killed_by_the_kernel` ran awk under `sh` that outlived the
killed child and exited 137 itself; Docker did not record `OOMKilled`, so the sandbox correctly reported `Exited(137)`
(product code behaves as designed: no guess is signed). Fix (test only): the hog is `exec`ed so it is the container's
own process; assertions were not loosened for Docker's record check (Docker still must yield MemoryLimit, Podman branches unchanged).
Evidence: commit c9a08a5; all 10 workflows for it completed with success, including OCI runtimes and cgroup v2
(all four jobs).
Not verified: whether Docker's OOMKilled stays reliable over many runs (one green run).
