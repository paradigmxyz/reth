macro_rules! stage_test_suite {
    ($runner:ident, $name:ident) => {

         paste::item! {
            /// Check that the execution is short-circuited if the database is empty.
            #[tokio::test]
            async fn [< execute_empty_db_ $name>] () {
                // Set up the runner
                let runner = $runner::default();

                // Execute the stage with empty database
                let input = crate::stage::ExecInput::default();

                // Run stage execution
                let result = runner.execute(input).await;
                runner.db().factory.static_file_provider().commit().unwrap();

                // Check that the result is returned and the stage does not panic.
                // The return result with empty db is stage-specific.
                assert_matches::assert_matches!(result, Ok(_));

                // Validate the stage execution
                assert_matches::assert_matches!(
                    runner.validate_execution(input, result.unwrap().ok()),
                    Ok(_),
                    "execution validation"
                );
            }

            // Run the complete stage execution flow.
            #[tokio::test]
            async fn [< execute_ $name>] () {
                let (target, current_checkpoint) = (500, 100);

                // Set up the runner
                let mut runner = $runner::default();
                let input = crate::stage::ExecInput {
                    target: Some(target),
                    checkpoint: Some(reth_primitives::stage::StageCheckpoint::new(current_checkpoint)),
                };
                let seed = runner.seed_execution(input).expect("failed to seed");
                let rx = runner.execute(input);

                // Run `after_execution` hook
                runner.after_execution(seed).await.expect("failed to run after execution hook");

                // Assert the successful result
                let result = rx.await.unwrap();
                runner.db().factory.static_file_provider().commit().unwrap();

                assert_matches::assert_matches!(
                    result,
                    Ok(ExecOutput { done, checkpoint })
                        if done && checkpoint.block_number == target
                );

                // Validate the stage execution
                assert_matches::assert_matches!(
                    runner.validate_execution(input, result.ok()),
                    Ok(_),
                    "execution validation"
                );
            }

            // Check that unwind does not panic on no new entries within the input range.
            #[tokio::test]
            async fn [< unwind_no_new_entries_ $name>] () {
                // Set up the runner
                let mut runner = $runner::default();
                let input = crate::stage::UnwindInput::default();

                // Seed the database
                runner.seed_execution(crate::stage::ExecInput::default()).expect("failed to seed");

                runner.before_unwind(input).expect("failed to execute before_unwind hook");

                // Run stage unwind
                let rx = runner.unwind(input).await;
                runner.db().factory.static_file_provider().commit().unwrap();

                assert_matches::assert_matches!(
                    rx,
                    Ok(UnwindOutput { checkpoint }) if checkpoint.block_number == input.unwind_to
                );

                // Validate the stage unwind
                assert_matches::assert_matches!(
                    runner.validate_unwind(input),
                    Ok(_),
                    "unwind validation"
                );
            }

            // Run complete execute and unwind flow.
            #[tokio::test]
            async fn [< unwind_ $name>] () {
                let (target, current_checkpoint) = (500, 100);

                // Set up the runner
                let mut runner = $runner::default();
                let execute_input = crate::stage::ExecInput {
                    target: Some(target),
                    checkpoint: Some(reth_primitives::stage::StageCheckpoint::new(current_checkpoint)),
                };
                let seed = runner.seed_execution(execute_input).expect("failed to seed");

                // Run stage execution
                let rx = runner.execute(execute_input);
                runner.after_execution(seed).await.expect("failed to run after execution hook");

                // Assert the successful execution result
                let result = rx.await.unwrap();
                runner.db().factory.static_file_provider().commit().unwrap();

                assert_matches::assert_matches!(
                    result,
                    Ok(ExecOutput { done, checkpoint })
                        if done && checkpoint.block_number == target
                );
                assert_matches::assert_matches!(
                    runner.validate_execution(execute_input, result.ok()),
                    Ok(_),
                    "execution validation"
                );


                // Run stage unwind
                let unwind_input = crate::stage::UnwindInput {
                    unwind_to: current_checkpoint,
                    checkpoint: reth_primitives::stage::StageCheckpoint::new(target),
                    bad_block: None,
                };

                runner.before_unwind(unwind_input).expect("Failed to unwind state");

                let rx = runner.unwind(unwind_input).await;
                // Assert the successful unwind result
                assert_matches::assert_matches!(
                    rx,
                    Ok(UnwindOutput { checkpoint }) if checkpoint.block_number == unwind_input.unwind_to
                );

                // Validate the stage unwind
                assert_matches::assert_matches!(
                    runner.validate_unwind(unwind_input),
                    Ok(_),
                    "unwind validation"
                );
            }
        }
    };
}

// `execute_already_reached_target` is not suitable for the headers stage thus
// included in the test suite extension
macro_rules! stage_test_suite_ext {
    ($runner:ident, $name:ident) => {
        crate::test_utils::stage_test_suite!($runner, $name);

        paste::item! {
            /// Check that the execution is short-circuited if the target was already reached.
            #[tokio::test]
            async fn [< execute_already_reached_target_ $name>] () {
                let current_checkpoint = 1000;

                // Set up the runner
                let mut runner = $runner::default();
                let input = crate::stage::ExecInput {
                    target: Some(current_checkpoint),
                    checkpoint: Some(reth_primitives::stage::StageCheckpoint::new(current_checkpoint)),
                };
                let seed = runner.seed_execution(input).expect("failed to seed");

                // Run stage execution
                let rx = runner.execute(input);

                // Run `after_execution` hook
                runner.after_execution(seed).await.expect("failed to run after execution hook");

                // Assert the successful result
                let result = rx.await.unwrap();
                runner.db().factory.static_file_provider().commit().unwrap();

                assert_matches::assert_matches!(
                    result,
                    Ok(ExecOutput { done, checkpoint })
                        if done && checkpoint.block_number == current_checkpoint
                );

                // Validate the stage execution
                assert_matches::assert_matches!(
                    runner.validate_execution(input, result.ok()),
                    Ok(_),
                    "execution validation"
                );
            }
        }
    };
}

pub(crate) use stage_test_suite;
pub(crate) use stage_test_suite_ext;
