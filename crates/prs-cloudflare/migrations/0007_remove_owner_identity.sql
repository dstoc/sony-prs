-- Cloudflare Access is the approval authorization boundary. The Worker no
-- longer maintains a second D1 owner-identity policy.
DROP TABLE IF EXISTS owner_identity;
