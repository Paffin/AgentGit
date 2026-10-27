// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { ApiError, describeError, errorFromResponse, parseRetryAfter } from './errors'

const headers = (h: Record<string, string> = {}) => new Headers(h)

describe('errorFromResponse', () => {
  it('reads the protocol envelope', () => {
    const e = errorFromResponse(
      409,
      {
        error: {
          code: 'stream_moved',
          message: 'compare-and-swap lost',
          retryable: true,
          retry_after_ms: 250,
          request_id: 'req_1',
          details: { stream: 'main' },
        },
      },
      headers(),
    )
    expect(e).toBeInstanceOf(ApiError)
    expect(e.code).toBe('stream_moved')
    expect(e.retryable).toBe(true)
    expect(e.retryAfterMs).toBe(250)
    expect(e.requestId).toBe('req_1')
    expect(e.details).toEqual({ stream: 'main' })
    expect(e.status).toBe(409)
  })

  it('takes the larger of the body hint and the Retry-After header', () => {
    const e = errorFromResponse(
      429,
      { error: { code: 'rate_limited', message: 'slow down', retry_after_ms: 500 } },
      headers({ 'retry-after': '3' }),
    )
    expect(e.retryAfterMs).toBe(3000)
  })

  it('does not trust the shape: an HTML gateway page still becomes a typed error', () => {
    const e = errorFromResponse(
      502,
      '<html>Bad gateway</html>',
      headers({ 'x-request-id': 'edge-9' }),
    )
    expect(e.code).toBe('unavailable')
    expect(e.retryable).toBe(true)
    expect(e.requestId).toBe('edge-9')
    expect(e.message).toContain('502')
  })

  it('maps unknown statuses without throwing', () => {
    expect(errorFromResponse(418, null, headers()).code).toBe('bad_response')
    expect(errorFromResponse(503, null, headers()).code).toBe('unavailable')
    expect(errorFromResponse(401, undefined, headers()).isUnauthenticated).toBe(true)
  })

  it('tolerates codes it has never heard of', () => {
    const e = errorFromResponse(400, { error: { code: 'brand_new_code', message: 'x' } }, headers())
    expect(e.code).toBe('brand_new_code')
    expect(e.retryable).toBe(false)
  })

  it('stringifies detail values', () => {
    const e = errorFromResponse(
      400,
      { error: { code: 'invalid_argument', message: 'm', details: { n: 3 } } },
      headers(),
    )
    expect(e.details.n).toBe('3')
  })
})

describe('parseRetryAfter', () => {
  it('parses delta-seconds', () => expect(parseRetryAfter('7')).toBe(7000))
  it('parses an HTTP date relative to now', () => {
    const now = Date.parse('2026-10-17T10:00:00Z')
    expect(parseRetryAfter('Sat, 17 Oct 2026 10:00:05 GMT', now)).toBe(5000)
    expect(parseRetryAfter('Sat, 17 Oct 2026 09:00:00 GMT', now)).toBe(0)
  })
  it('ignores junk', () => {
    expect(parseRetryAfter(null)).toBe(0)
    expect(parseRetryAfter('soon')).toBe(0)
  })
})

describe('describeError', () => {
  it('speaks plainly about network failures and rate limits', () => {
    expect(describeError(new ApiError({ status: 0, code: 'network_error', message: 'x' }))).toMatch(
      /Cannot reach/,
    )
    expect(
      describeError(
        new ApiError({ status: 429, code: 'rate_limited', message: 'x', retryAfterMs: 2500 }),
      ),
    ).toContain('3s')
  })
  it('uses the server message otherwise, and survives non-errors', () => {
    expect(
      describeError(new ApiError({ status: 400, code: 'invalid_argument', message: 'bad name' })),
    ).toBe('bad name')
    expect(describeError('weird')).toBe('Something went wrong.')
    expect(describeError(new Error('boom'))).toBe('boom')
  })
})
