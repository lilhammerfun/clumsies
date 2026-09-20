// Install a local fake-OIDC session through the daemon's native login boundary.
// Read credentials from stdin, never command arguments or a file.
import Foundation
import XPC

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

guard CommandLine.arguments.count == 2,
      CommandLine.arguments[1].range(of: "^[a-f0-9]{12}$", options: .regularExpression) != nil,
      let payload = try JSONSerialization.jsonObject(
        with: FileHandle.standardInput.readDataToEndOfFile()) as? [String: String],
      let origin = payload["server_url"], let url = URL(string: origin),
      url.scheme == "http", url.host == "127.0.0.1", url.port != nil,
      payload["access_token"]?.isEmpty == false, payload["refresh_token"]?.isEmpty == false else {
    fail("Dev login requires a local instance and a loopback session on stdin.")
}
let connection = xpc_connection_create_mach_service(
    "ai.clumsies.daemon.dev.\(CommandLine.arguments[1])", .main, 0)
xpc_connection_set_event_handler(connection) { _ in }
xpc_connection_activate(connection)
let data = try JSONSerialization.data(withJSONObject: [
    "method": "replace_project_config", "payload": payload, "request_id": UUID().uuidString
])
let request = xpc_dictionary_create(nil, nil, 0)
xpc_dictionary_set_string(request, "request_json", String(decoding: data, as: UTF8.self))
xpc_connection_send_message_with_reply(connection, request, .main) { reply in
    guard xpc_get_type(reply) == XPC_TYPE_DICTIONARY,
          let raw = xpc_dictionary_get_string(reply, "response_json"),
          let result = try? JSONSerialization.jsonObject(
            with: Data(String(cString: raw).utf8)) as? [String: Any],
          result["ok"] as? Bool == true else {
        fail("Could not install the local Dev session through the daemon.")
    }
    print("Local Dev App signed in.")
    exit(0)
}
DispatchQueue.main.asyncAfter(deadline: .now() + 30) { fail("Dev login timed out.") }
dispatchMain()
