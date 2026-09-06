-- A batch the server refuses outright used to block the outbox head forever:
-- every reconnect resent the same operation and got the same rejection, so no
-- later change reached the server either. Quarantining the offender keeps it
-- (nothing is ever deleted from the outbox) while letting the queue drain.
ALTER TABLE sync_outbox ADD COLUMN quarantined_at INTEGER;
ALTER TABLE sync_outbox ADD COLUMN quarantine_reason TEXT;
