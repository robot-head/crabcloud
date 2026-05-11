//! Nextcloud OCS status codes. Hand-mapped to match upstream behavior so
//! existing clients see the numbers they expect.
//!
//! See spec §9.3.

/// OCS-level status (the `<statuscode>` in the envelope). Distinct from the
/// HTTP status — both are emitted, the OCS one inside the envelope, the
/// HTTP one in the wire-level response status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcsStatus {
    /// Successful operation. `100` in OCS v1, `200` in v2.
    Ok,
    /// Resource created. `201` in v2; collapses to `Ok` in v1.
    Created,
    /// Client sent a malformed request (`400`).
    BadRequest,
    /// Authentication required or rejected (Nextcloud-specific `997`).
    Unauthorized,
    /// Authentication accepted but action denied (`403`).
    Forbidden,
    /// Target resource missing (Nextcloud-specific `998`).
    NotFound,
    /// Catch-all client-visible failure (`999`).
    UnknownError,
    /// Internal server error (Nextcloud-specific `996`).
    ServerError,
}

impl OcsStatus {
    /// Numeric `statuscode` value for OCS v1 envelopes.
    pub fn v1_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 100,
            OcsStatus::Created => 100,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 997,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 998,
            OcsStatus::UnknownError => 999,
            OcsStatus::ServerError => 996,
        }
    }

    /// Numeric `statuscode` value for OCS v2 envelopes.
    pub fn v2_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 200,
            OcsStatus::Created => 201,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 997,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 998,
            OcsStatus::UnknownError => 999,
            OcsStatus::ServerError => 996,
        }
    }

    /// Short `status` string (`"ok"` or `"failure"`) shown in the envelope.
    pub fn label(self) -> &'static str {
        match self {
            OcsStatus::Ok => "ok",
            OcsStatus::Created => "ok",
            OcsStatus::BadRequest => "failure",
            OcsStatus::Unauthorized => "failure",
            OcsStatus::Forbidden => "failure",
            OcsStatus::NotFound => "failure",
            OcsStatus::UnknownError => "failure",
            OcsStatus::ServerError => "failure",
        }
    }

    /// Wire-level HTTP status code that should accompany the envelope.
    pub fn http_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 200,
            OcsStatus::Created => 201,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 401,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 404,
            OcsStatus::UnknownError => 500,
            OcsStatus::ServerError => 500,
        }
    }
}

/// Which OCS protocol version the response is wrapped in. Affects only
/// the `statuscode` mapping (100 vs 200 for OK).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcsVersion {
    /// OCS v1 protocol; OK is `100`.
    V1,
    /// OCS v2 protocol; OK is `200`.
    V2,
}

impl OcsStatus {
    /// Returns the numeric statuscode appropriate for the given OCS protocol version.
    pub fn code_for(self, version: OcsVersion) -> u16 {
        match version {
            OcsVersion::V1 => self.v1_code(),
            OcsVersion::V2 => self.v2_code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_maps_to_100_in_v1_and_200_in_v2() {
        assert_eq!(OcsStatus::Ok.code_for(OcsVersion::V1), 100);
        assert_eq!(OcsStatus::Ok.code_for(OcsVersion::V2), 200);
    }

    #[test]
    fn nextcloud_specific_failure_codes_match_upstream() {
        assert_eq!(OcsStatus::Unauthorized.v2_code(), 997);
        assert_eq!(OcsStatus::NotFound.v2_code(), 998);
        assert_eq!(OcsStatus::UnknownError.v2_code(), 999);
        assert_eq!(OcsStatus::ServerError.v2_code(), 996);
    }

    #[test]
    fn http_codes_independent_from_ocs_codes() {
        assert_eq!(OcsStatus::Unauthorized.http_code(), 401);
        assert_eq!(OcsStatus::NotFound.http_code(), 404);
    }
}
