package metrics

// Service is the catalogue of metrics the orchestrator exports. Components
// receive a *Service (never nil; use NewService(NewRegistry()) in tests) and
// increment what they own.
type Service struct {
	Registry *Registry

	SessionsStarted   *CounterVec // labels: repo, executor
	SessionsFinished  *CounterVec // labels: repo, executor, outcome (completed|failed|timeout|cancelled|lost)
	SessionsRunning   *GaugeVec   // labels: repo, executor
	TasksDeferred     *CounterVec // labels: repo, reason
	ReconcileLoops    *CounterVec // labels: repo, component
	ReconcileErrors   *CounterVec // labels: repo, component
	EventsReceived    *CounterVec // labels: repo, topic
	SSEReconnects     *CounterVec // labels: repo
	APIRequests       *CounterVec // labels: identity, operation
	APIErrors         *CounterVec // labels: code
	APIRetries        *CounterVec // labels: code
	ValidationsQueued *CounterVec // labels: repo
	Preparations      *CounterVec // labels: repo
	AdvisorSuggest    *CounterVec // labels: repo, kind
	ReviewsPosted     *CounterVec // labels: repo, reviewer, finding
	IndexFiles        *GaugeVec   // labels: repo
	IndexSymbols      *GaugeVec   // labels: repo
	IndexUpdates      *CounterVec // labels: repo
	CredentialsMinted *CounterVec // labels: repo
	CredentialExpiry  *GaugeVec   // labels: identity (unix seconds at which the service token expires)
}

// ObserveRequest, ObserveError and ObserveRetry make *Service an agwp.Observer.
func (s *Service) ObserveRequest(identity, operation string) {
	s.APIRequests.With(identity, operation).Inc()
}

// ObserveError counts an AGWP error by code ("transport" for failures below HTTP).
func (s *Service) ObserveError(code string) { s.APIErrors.With(code).Inc() }

// ObserveRetry counts a retried request by the code that caused the retry.
func (s *Service) ObserveRetry(code string) { s.APIRetries.With(code).Inc() }

// NewService registers the catalogue on r.
func NewService(r *Registry) *Service {
	return &Service{
		Registry:          r,
		SessionsStarted:   r.NewCounterVec("orchestrator_sessions_started_total", "Agent sessions started.", "repo", "executor"),
		SessionsFinished:  r.NewCounterVec("orchestrator_sessions_finished_total", "Agent sessions that ended, by outcome.", "repo", "executor", "outcome"),
		SessionsRunning:   r.NewGaugeVec("orchestrator_sessions_running", "Agent sessions currently running.", "repo", "executor"),
		TasksDeferred:     r.NewCounterVec("orchestrator_tasks_deferred_total", "Scheduling passes in which a ready task was deferred.", "repo", "reason"),
		ReconcileLoops:    r.NewCounterVec("orchestrator_reconcile_loops_total", "Completed reconcile loops.", "repo", "component"),
		ReconcileErrors:   r.NewCounterVec("orchestrator_reconcile_errors_total", "Reconcile loops that ended with an error.", "repo", "component"),
		EventsReceived:    r.NewCounterVec("orchestrator_events_received_total", "Repository events received over SSE.", "repo", "topic"),
		SSEReconnects:     r.NewCounterVec("orchestrator_sse_reconnects_total", "SSE reconnections.", "repo"),
		APIRequests:       r.NewCounterVec("orchestrator_agwp_requests_total", "AGWP requests sent.", "identity", "operation"),
		APIErrors:         r.NewCounterVec("orchestrator_agwp_errors_total", "AGWP errors received, by error code.", "code"),
		APIRetries:        r.NewCounterVec("orchestrator_agwp_retries_total", "AGWP requests retried, by error code.", "code"),
		ValidationsQueued: r.NewCounterVec("orchestrator_validations_queued_total", "Preparations that asked the server to queue validations.", "repo"),
		Preparations:      r.NewCounterVec("orchestrator_preparations_total", "integration.prepare calls made for open proposals.", "repo"),
		AdvisorSuggest:    r.NewCounterVec("orchestrator_advisor_suggestions_total", "Conflict-advisor suggestions computed.", "repo", "kind"),
		ReviewsPosted:     r.NewCounterVec("orchestrator_reviews_posted_total", "Comment reviews posted by the automated reviewers.", "repo", "reviewer", "finding"),
		IndexFiles:        r.NewGaugeVec("orchestrator_index_files", "Files in the semantic index.", "repo"),
		IndexSymbols:      r.NewGaugeVec("orchestrator_index_symbols", "Symbols in the semantic index.", "repo"),
		IndexUpdates:      r.NewCounterVec("orchestrator_index_updates_total", "Incremental index updates.", "repo"),
		CredentialsMinted: r.NewCounterVec("orchestrator_credentials_minted_total", "Short-lived credentials minted for agents.", "repo"),
		CredentialExpiry:  r.NewGaugeVec("orchestrator_service_token_expiry_seconds", "Unix time at which the orchestrator's own credential expires (0 = never).", "identity"),
	}
}
