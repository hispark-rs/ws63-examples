//! Allocation-free DNS query and response validation for the HIL contract.

pub const QUERY_NAME: &[u8] = b"\x07example\x03com\x00";
pub const QUERY_LEN: usize = 12 + QUERY_NAME.len() + 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidResponse {
    pub answer_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseError {
    Truncated,
    TransactionId,
    NotResponse,
    Opcode,
    TruncatedResponse,
    ResponseCode,
    QuestionCount,
    Question,
    NoAnswers,
}

pub fn build_query(transaction_id: u16) -> [u8; QUERY_LEN] {
    let mut query = [0_u8; QUERY_LEN];
    query[..2].copy_from_slice(&transaction_id.to_be_bytes());
    query[2..4].copy_from_slice(&0x0100_u16.to_be_bytes());
    query[4..6].copy_from_slice(&1_u16.to_be_bytes());
    query[12..12 + QUERY_NAME.len()].copy_from_slice(QUERY_NAME);
    let question_tail = 12 + QUERY_NAME.len();
    query[question_tail..question_tail + 2].copy_from_slice(&1_u16.to_be_bytes());
    query[question_tail + 2..question_tail + 4].copy_from_slice(&1_u16.to_be_bytes());
    query
}

pub fn validate_response(
    response: &[u8],
    transaction_id: u16,
) -> Result<ValidResponse, ResponseError> {
    if response.len() < QUERY_LEN {
        return Err(ResponseError::Truncated);
    }
    if u16::from_be_bytes([response[0], response[1]]) != transaction_id {
        return Err(ResponseError::TransactionId);
    }

    let flags = u16::from_be_bytes([response[2], response[3]]);
    if flags & 0x8000 == 0 {
        return Err(ResponseError::NotResponse);
    }
    if flags & 0x7800 != 0 {
        return Err(ResponseError::Opcode);
    }
    if flags & 0x0200 != 0 {
        return Err(ResponseError::TruncatedResponse);
    }
    if flags & 0x000f != 0 {
        return Err(ResponseError::ResponseCode);
    }
    if u16::from_be_bytes([response[4], response[5]]) != 1 {
        return Err(ResponseError::QuestionCount);
    }

    let expected_question = build_query(transaction_id);
    if response[12..QUERY_LEN] != expected_question[12..QUERY_LEN] {
        return Err(ResponseError::Question);
    }

    let answer_count = u16::from_be_bytes([response[6], response[7]]);
    if answer_count == 0 {
        return Err(ResponseError::NoAnswers);
    }
    Ok(ValidResponse { answer_count })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_response(transaction_id: u16) -> [u8; QUERY_LEN + 16] {
        let query = build_query(transaction_id);
        let mut response = [0_u8; QUERY_LEN + 16];
        response[..QUERY_LEN].copy_from_slice(&query);
        response[2..4].copy_from_slice(&0x8180_u16.to_be_bytes());
        response[6..8].copy_from_slice(&1_u16.to_be_bytes());
        response[QUERY_LEN..].copy_from_slice(&[
            0xc0, 0x0c, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 0xc0, 0x00,
            0x02, 0x01,
        ]);
        response
    }

    #[test]
    fn builds_a_stable_example_com_a_query() {
        assert_eq!(
            build_query(0x5753),
            [
                0x57, 0x53, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, b'e',
                b'x', b'a', b'm', b'p', b'l', b'e', 0x03, b'c', b'o', b'm', 0x00, 0x00, 0x01, 0x00,
                0x01,
            ]
        );
    }

    #[test]
    fn accepts_a_matching_successful_response_with_answers() {
        assert_eq!(
            validate_response(&valid_response(0x5753), 0x5753),
            Ok(ValidResponse { answer_count: 1 })
        );
    }

    #[test]
    fn rejects_stale_and_unsuccessful_responses() {
        let mut stale = valid_response(0x5753);
        stale[..2].copy_from_slice(&0x5754_u16.to_be_bytes());
        assert_eq!(
            validate_response(&stale, 0x5753),
            Err(ResponseError::TransactionId)
        );

        let mut nxdomain = valid_response(0x5753);
        nxdomain[3] = 0x83;
        assert_eq!(
            validate_response(&nxdomain, 0x5753),
            Err(ResponseError::ResponseCode)
        );

        let mut no_answers = valid_response(0x5753);
        no_answers[6..8].copy_from_slice(&0_u16.to_be_bytes());
        assert_eq!(
            validate_response(&no_answers, 0x5753),
            Err(ResponseError::NoAnswers)
        );
    }
}
