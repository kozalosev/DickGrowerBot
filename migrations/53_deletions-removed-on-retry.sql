-- A message found gone after our own request timed out: most likely that request deleted it.
ALTER TYPE deletion_state ADD VALUE IF NOT EXISTS 'removed_on_retry' AFTER 'removed_before';

-- Whether the last request about the message timed out, so it may have been carried out.
ALTER TABLE Scheduled_Message_Deletions ADD COLUMN IF NOT EXISTS timed_out boolean NOT NULL DEFAULT false;
