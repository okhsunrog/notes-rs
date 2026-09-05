package dev.okhsunrog.mobile_system

/** Reject stale Chromium acknowledgements and submissions superseded by a pen gesture. */
internal class InkFrameFence {
    data class Submission(val revision: Long, val generation: Long)
    var revision = 0L
        private set
    private var readyRevision = -1L
    private var readySequence = -1L
    private var presentedRevision = -1L
    private var generation = 0L

    fun request(): Long {
        cancelSubmission()
        return ++revision
    }

    fun ready(request: Long, sequence: Long): Boolean {
        if (request != revision) return false
        readyRevision = request
        readySequence = sequence
        return true
    }

    fun canSubmit(sequence: Long, drawing: Boolean): Boolean =
        !drawing && readyRevision == revision && readySequence >= sequence && presentedRevision != revision

    fun submission(): Submission = Submission(revision, ++generation)
    fun cancelSubmission() { generation++ }
    fun isCurrent(ticket: Submission): Boolean = ticket.revision == revision && ticket.generation == generation

    fun present(ticket: Submission, sequence: Long, drawing: Boolean): Boolean {
        if (!isCurrent(ticket) || !canSubmit(sequence, drawing)) return false
        presentedRevision = ticket.revision
        return true
    }
}
