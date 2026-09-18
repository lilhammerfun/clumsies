import Foundation

enum JSONCoding {
    static func encoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        return encoder
    }

    static func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }
}

struct EmptyPayload: Codable, Sendable {}

struct APIErrorPayload: Codable, Error, Equatable, Sendable {
    let code: String
    let message: String
    let requestId: String?
    let details: APIErrorDetails?

    init(code: String, message: String, requestId: String?, details: APIErrorDetails? = nil) {
        self.code = code
        self.message = message
        self.requestId = requestId
        self.details = details
    }

    private enum CodingKeys: String, CodingKey { case code, message, requestId, details }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        code = try values.decode(String.self, forKey: .code)
        message = try values.decode(String.self, forKey: .message)
        requestId = try values.decodeIfPresent(String.self, forKey: .requestId)
        details = try? values.decodeIfPresent(APIErrorDetails.self, forKey: .details)
    }
}

struct APIErrorDetails: Codable, Equatable, Sendable {
    struct Cause: Codable, Equatable, Sendable { let kind: String; let osCode: Int? }
    let timeout: Bool?
    let connect: Bool?
    let body: Bool?
    let decode: Bool?
    let status: Int?
    let causes: [Cause]?
}
