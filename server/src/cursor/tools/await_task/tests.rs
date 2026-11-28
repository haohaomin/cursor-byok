use super::*;
use crate::cursor::tools::{
    codec,
    tool_call_result::{tool_result_channel, ToolCompletion, ToolResultReceiver},
};
use pb::{
    agent_server_message::Message as Server, exec_client_message::Message as ResponseMessage,
    exec_server_message::Message as Request,
};
use serde_json::json;

fn call(args: Value) -> ToolCall {
    ToolCall {
        index: 0,
        call_id: "await-call".into(),
        model_call_id: "model".into(),
        name: "Await".into(),
        arguments_text: args.to_string(),
        arguments: args,
        argument_error: None,
    }
}

async fn begin(args: Value) -> (CursorToolRuntime, ToolResultReceiver, ToolStart) {
    let runtime = CursorToolRuntime::default();
    let (sender, receiver) = tool_result_channel();
    let context = ExecContext {
        terminals_folder: "/client/terminals".into(),
        allow_subagents: true,
        ..Default::default()
    };
    let started = start(&runtime, &sender, &call(args), &context)
        .await
        .unwrap();
    (runtime, receiver, started)
}

fn exec(message: &pb::AgentServerMessage) -> &pb::ExecServerMessage {
    let Some(Server::ExecServerMessage(exec)) = message.message.as_ref() else {
        panic!("expected client Exec")
    };
    exec
}

async fn reply(
    runtime: &CursorToolRuntime,
    request: &pb::AgentServerMessage,
    message: ResponseMessage,
) {
    let event = codec::client_event(
        &pb::ExecClientMessage {
            id: exec(request).id,
            message: Some(message),
            ..Default::default()
        },
        runtime,
    )
    .await
    .unwrap();
    assert!(matches!(event, codec::ClientExecEvent::Pending));
}

fn read(text: &str) -> ResponseMessage {
    ResponseMessage::ReadResult(pb::ReadResult {
        result: Some(pb::read_result::Result::Success(pb::ReadSuccess {
            output: Some(pb::read_success::Output::Content(text.into())),
            file_size: text.len() as i64,
            ..Default::default()
        })),
    })
}

async fn done(receiver: &mut ToolResultReceiver) -> ToolCompletion {
    tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

fn success(completion: &ToolCompletion) -> &pb::await_success::AwaitResult {
    assert!(
        !completion.result().is_error,
        "{}",
        completion.result().content
    );
    let Some(pb::tool_call::Tool::AwaitToolCall(tool)) = completion.tool_call().tool.as_ref()
    else {
        panic!("Await card missing")
    };
    let Some(pb::await_result::Result::Success(result)) =
        tool.result.as_ref().unwrap().result.as_ref()
    else {
        panic!("Await success missing")
    };
    result.await_result.as_ref().unwrap()
}

#[tokio::test]
async fn shell_completion_is_read_from_footer_and_delivered_once() {
    let (runtime, mut receiver, started) = begin(json!({"task_id":"007","block_until_ms":0})).await;
    let request = &started.messages[0];
    let Some(Request::ReadArgs(args)) = exec(request).message.as_ref() else {
        panic!("not a Read")
    };
    assert_eq!(args.path, "/client/terminals/007.txt");
    let text = "---\npid: 10\n---\nhello\n---\nexit_code: -2\nelapsed_ms: 345\n---\n";
    reply(&runtime, request, read(text)).await;
    let completion = done(&mut receiver).await;
    let pb::await_success::AwaitResult::Complete(value) = success(&completion) else {
        panic!("not complete")
    };
    assert_eq!(value.exit_code, Some(-2));
    assert_eq!(value.runtime_ms, 345);
    assert_eq!(value.output_length, text.len() as u64);
    reply(&runtime, request, read(text)).await;
    assert!(receiver.try_recv().is_none());
    assert!(runtime.running_exec_ids().await.is_empty());
}

#[tokio::test]
async fn shell_regex_only_matches_body_and_returns_still_running() {
    let (runtime, mut receiver, started) =
        begin(json!({"task_id":"12","block_until_ms":0,"pattern":"^READY$"})).await;
    reply(
        &runtime,
        &started.messages[0],
        read("---\nREADY\n---\nwaiting\n---\nREADY\n---\n"),
    )
    .await;
    let result = done(&mut receiver).await;
    let pb::await_success::AwaitResult::StillRunning(value) = success(&result) else {
        panic!("not running")
    };
    assert_eq!(value.regex_match, None);
    let (runtime, mut receiver, started) =
        begin(json!({"task_id":"12","block_until_ms":5000,"pattern":"^READY$"})).await;
    reply(
        &runtime,
        &started.messages[0],
        read("---\npid: 1\n---\nREADY\nwaiting"),
    )
    .await;
    let result = done(&mut receiver).await;
    let pb::await_success::AwaitResult::StillRunning(value) = success(&result) else {
        panic!("not running")
    };
    assert_eq!(value.regex_match.as_deref(), Some("READY"));
}

#[tokio::test]
async fn shell_poll_uses_another_read_and_does_not_block_cancellation() {
    let (runtime, mut receiver, started) =
        begin(json!({"task_id":"12","block_until_ms":5000})).await;
    reply(&runtime, &started.messages[0], read("working")).await;
    let next = tokio::time::timeout(Duration::from_secs(1), runtime.next_message())
        .await
        .unwrap();
    assert_ne!(exec(&next).id, exec(&started.messages[0]).id);
    assert!(matches!(exec(&next).message, Some(Request::ReadArgs(_))));
    let aborts = runtime.interrupt_for_message().await;
    assert_eq!(aborts.len(), 1);
    let Some(Server::ExecServerControlMessage(control)) = aborts[0].message.as_ref() else {
        panic!("not an abort")
    };
    assert!(
        matches!(&control.message,Some(pb::exec_server_control_message::Message::Abort(abort)) if abort.id==exec(&next).id)
    );
    reply(&runtime, &next, read("late\n---\nexit_code: 0\n---")).await;
    tokio::task::yield_now().await;
    assert!(receiver.try_recv().is_none());
    assert!(runtime.running_exec_ids().await.is_empty());
}

#[tokio::test]
async fn cancelled_timer_and_queued_read_never_publish_late_work() {
    let (runtime, mut receiver, _) = begin(json!({"block_until_ms":30})).await;
    assert!(runtime.interrupt_for_message().await.is_empty());
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(receiver.try_recv().is_none());
    let (runtime, mut receiver, started) =
        begin(json!({"task_id":"12","block_until_ms":5000})).await;
    reply(&runtime, &started.messages[0], read("working")).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(runtime.interrupt_for_run_replacement().await.len(), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), runtime.next_message())
            .await
            .is_err()
    );
    assert!(receiver.try_recv().is_none());
}

#[tokio::test]
async fn pure_wait_finishes_without_client_execution() {
    let (runtime, mut receiver, started) = begin(json!({"block_until_ms":5})).await;
    assert!(started.messages.is_empty());
    let completion = done(&mut receiver).await;
    let pb::await_success::AwaitResult::Complete(value) = success(&completion) else {
        panic!("not complete")
    };
    assert_eq!(value.runtime_ms, 5);
    assert_eq!(value.task_id, "");
    assert!(runtime.running_exec_ids().await.is_empty());
}

#[tokio::test]
async fn subagent_completion_retains_status_if_transcript_read_fails() {
    for finished in [true, false] {
        let (runtime, mut receiver, started) =
            begin(json!({"task_id":"child","block_until_ms":50})).await;
        let result = if finished {
            pb::subagent_await_result::Result::Complete(pb::SubagentAwaitComplete {
                agent_id: "child".into(),
                transcript_path: Some("/client/child.jsonl".into()),
                final_message: Some("private final body".into()),
                ..Default::default()
            })
        } else {
            pb::subagent_await_result::Result::StillRunning(pb::SubagentAwaitStillRunning {
                agent_id: "child".into(),
                transcript_path: Some("/client/child.jsonl".into()),
            })
        };
        reply(
            &runtime,
            &started.messages[0],
            ResponseMessage::SubagentAwaitResult(pb::SubagentAwaitResult {
                result: Some(result),
            }),
        )
        .await;
        let transcript = tokio::time::timeout(Duration::from_secs(1), runtime.next_message())
            .await
            .unwrap();
        assert!(
            matches!(&exec(&transcript).message,Some(Request::ReadArgs(args)) if args.path=="/client/child.jsonl")
        );
        reply(
            &runtime,
            &transcript,
            ResponseMessage::ReadResult(pb::ReadResult {
                result: Some(pb::read_result::Result::FileNotFound(
                    pb::ReadFileNotFound::default(),
                )),
            }),
        )
        .await;
        let completion = done(&mut receiver).await;
        assert_eq!(
            matches!(
                success(&completion),
                pb::await_success::AwaitResult::Complete(_)
            ),
            finished
        );
        assert!(!completion.result().content.contains("private final body"));
        assert!(completion.result().content.contains("\"output_length\":0"));
    }
}

#[tokio::test]
async fn missing_child_and_closed_exec_are_tool_errors() {
    for closed in [false, true] {
        let (runtime, mut receiver, started) =
            begin(json!({"task_id":"missing-child","block_until_ms":0})).await;
        if closed {
            assert!(matches!(
                codec::stream_closed(exec(&started.messages[0]).id, &runtime)
                    .await
                    .unwrap(),
                codec::ClientExecEvent::Pending
            ));
        } else {
            reply(
                &runtime,
                &started.messages[0],
                ResponseMessage::SubagentAwaitResult(pb::SubagentAwaitResult {
                    result: Some(pb::subagent_await_result::Result::NotFound(
                        pb::SubagentAwaitNotFound {
                            agent_id: "missing-child".into(),
                        },
                    )),
                }),
            )
            .await;
        }
        let completion = done(&mut receiver).await;
        assert!(completion.result().is_error);
        assert!(matches!(
            completion.tool_call().tool,
            Some(pb::tool_call::Tool::AwaitToolCall(_))
        ));
        assert!(runtime.running_exec_ids().await.is_empty());
    }
}

#[tokio::test]
async fn truncated_shell_output_is_not_reported_as_running() {
    let (runtime, mut receiver, started) = begin(json!({"task_id":"12","block_until_ms":0})).await;
    reply(
        &runtime,
        &started.messages[0],
        ResponseMessage::ReadResult(pb::ReadResult {
            result: Some(pb::read_result::Result::Success(pb::ReadSuccess {
                truncated: true,
                output: Some(pb::read_success::Output::Content("partial".into())),
                ..Default::default()
            })),
        }),
    )
    .await;
    let completion = done(&mut receiver).await;
    assert!(completion.result().is_error);
    assert!(completion.result().content.contains("truncated"));
}

#[test]
fn parameter_rules_bound_waits_and_reject_invalid_targets() {
    assert_eq!(
        arguments(&call(json!({"block_until_ms":-1})))
            .unwrap()
            .wait_ms,
        30_000
    );
    assert_eq!(
        arguments(&call(json!({"block_until_ms":1e20})))
            .unwrap()
            .wait_ms,
        7_140_000
    );
    assert_eq!(
        arguments(&call(json!({"block_until_ms":5.9})))
            .unwrap()
            .wait_ms,
        5
    );
    for args in [
        json!({"block_until_ms":0}),
        json!({"task_id":5}),
        json!({"taskId":"misspelled"}),
        json!({"task_id":"child","pattern":"ready"}),
    ] {
        assert!(arguments(&call(args)).is_err());
    }
}

#[tokio::test]
async fn validation_failures_keep_await_cards_without_starting_client_work() {
    use crate::cursor::tools::{ToolBatchState, ToolDispatcher};
    use prost::Message;
    use std::collections::{BTreeMap, HashSet};

    for (args, terminals, allow_subagents, expected_error, valid_args) in [
        (json!({"task_id":5}), "", true, "must be a string", false),
        (
            json!({"task_id":"child","pattern":"ready"}),
            "",
            true,
            "only supported for Shell",
            false,
        ),
        (json!({"task_id":"12"}), "", true, "terminals folder", true),
        (
            json!({"task_id":"12","pattern":"["}),
            "/terminals",
            true,
            "Invalid Await pattern",
            true,
        ),
        (json!({"task_id":"child"}), "", false, "unavailable", true),
        (
            json!({"block_until_ms":0}),
            "",
            true,
            "greater than zero",
            false,
        ),
    ] {
        let runtime = CursorToolRuntime::default();
        let dispatcher = ToolDispatcher::new(runtime.clone());
        let call = call(args);
        let dispatched = dispatcher
            .start_batch(
                std::slice::from_ref(&call),
                ToolBatchState {
                    completed: &HashSet::new(),
                    started: &HashSet::new(),
                    response_text: "",
                    response_thinking: "",
                    goal_state: None,
                },
                &[],
                &BTreeMap::new(),
                &ExecContext {
                    terminals_folder: terminals.into(),
                    allow_subagents,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(dispatched.len(), 1);
        assert!(dispatched[0].messages.is_empty());
        let completion = dispatched[0]
            .completion
            .as_ref()
            .expect("validation result");
        assert!(completion.result().is_error);
        assert_eq!(completion.result().call_id, call.call_id);
        assert!(
            completion.result().content.contains(expected_error),
            "{}",
            completion.result().content
        );
        let wire = pb::ToolCall::decode(completion.tool_call().encode_to_vec().as_slice()).unwrap();
        assert_eq!(wire.tool_call_id.as_deref(), Some(call.call_id.as_str()));
        let Some(pb::tool_call::Tool::AwaitToolCall(tool)) = wire.tool else {
            panic!("validation failure must remain an Await card")
        };
        assert_eq!(tool.args.is_some(), valid_args);
        let Some(pb::await_result::Result::Error(error)) = tool.result.and_then(|r| r.result)
        else {
            panic!("Await validation error missing")
        };
        assert_eq!(error.error, completion.result().content);
        assert!(runtime.running_exec_ids().await.is_empty());
    }
}

#[tokio::test]
async fn unexpected_stream_result_fails_await_once() {
    let (runtime, mut receiver, started) =
        begin(json!({"task_id":"child","block_until_ms":0})).await;
    let response = ResponseMessage::ShellStream(pb::ShellStream {
        event: Some(pb::shell_stream::Event::Stdout(pb::ShellStreamStdout {
            data: "unexpected".into(),
        })),
    });
    reply(&runtime, &started.messages[0], response.clone()).await;
    let completion = done(&mut receiver).await;
    assert!(completion.result().is_error);
    assert!(completion.result().content.contains("SubagentAwaitResult"));
    reply(&runtime, &started.messages[0], response).await;
    assert!(receiver.try_recv().is_none());
    assert!(runtime.running_exec_ids().await.is_empty());
}

#[tokio::test]
async fn cancelling_subagent_await_aborts_only_the_observer() {
    let (runtime, mut receiver, started) =
        begin(json!({"task_id":"child","block_until_ms":5000})).await;
    let aborts = runtime.drain_running().await;
    assert_eq!(aborts.len(), 1);
    let Some(Server::ExecServerControlMessage(control)) = aborts[0].message.as_ref() else {
        panic!("not an Exec abort")
    };
    assert!(
        matches!(&control.message, Some(pb::exec_server_control_message::Message::Abort(abort)) if abort.id == exec(&started.messages[0]).id)
    );
    tokio::task::yield_now().await;
    assert!(receiver.try_recv().is_none());
    assert!(runtime.running_exec_ids().await.is_empty());
}

#[tokio::test]
async fn terminal_metadata_parsing_preserves_output_and_empty_completion() {
    let (runtime, mut receiver, started) =
        begin(json!({"task_id":"12","block_until_ms":0,"pattern":"READY\\n"})).await;
    reply(
        &runtime,
        &started.messages[0],
        read("---\npid: 10\n---\nREADY\n"),
    )
    .await;
    let completion = done(&mut receiver).await;
    let pb::await_success::AwaitResult::StillRunning(value) = success(&completion) else {
        panic!("not running")
    };
    assert_eq!(value.regex_match.as_deref(), Some("READY\n"));

    for gap in ["", "\n"] {
        let (runtime, mut receiver, started) =
            begin(json!({"task_id":"12","block_until_ms":0})).await;
        reply(
            &runtime,
            &started.messages[0],
            read(&format!(
                "---\npid: 10\n---\n{gap}---\nexit_code: 0\nelapsed_ms: 45\n---\n"
            )),
        )
        .await;
        let completion = done(&mut receiver).await;
        let pb::await_success::AwaitResult::Complete(value) = success(&completion) else {
            panic!("empty output was not complete")
        };
        assert_eq!(value.exit_code, Some(0));
        assert_eq!(value.runtime_ms, 45);
    }
}

#[tokio::test]
async fn running_shell_reports_task_age_for_a_zero_wait_check() {
    let (runtime, mut receiver, started) = begin(json!({"task_id":"12","block_until_ms":0})).await;
    reply(
        &runtime,
        &started.messages[0],
        read("---\nrunning_for_ms: 60000\nstarted_at: 2000-01-01T00:00:00.000Z\n---\nworking\n"),
    )
    .await;
    let completion = done(&mut receiver).await;
    let pb::await_success::AwaitResult::StillRunning(value) = success(&completion) else {
        panic!("not running")
    };
    assert!((60_000..61_000).contains(&value.runtime_ms));
}
