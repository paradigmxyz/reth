macro_rules! stage_test_suite {
    ($runner:ident) => {
        /// Check that the execution is short-circuited if the database is empty.
        #[tokio::test]
        async fn execute_empty_db() {
            // Set up the runner
            let runner = $runner::default();

            // Execute the stage with empty database
            let input = crate::stage::ExecInput::default();

            // Run stage execution
            let result = runner.execute(input).await;
            // Check that the result is returned and the stage does not panic.
            // The return result with empty db is stage-specific.
            assert_matches::assert_matches!(result, Ok(_));

            // Validate the stage execution
            assert!(runner.validate_execution(input, result.unwrap().ok()).is_ok(), "execution validation");
        }

        // Run the complete stage execution flow.
        #[tokio::test]
        async fn execute() {
            let (previous_stage, stage_progress) = (500, 100);

            // Set up the runner
            let mut runner = $runner::default();
            let input = crate::stage::ExecInput {
                previous_stage: Some((crate::test_utils::PREV_STAGE_ID, previous_stage)),
                stage_progress: Some(stage_progress),
            };
            let seed = runner.seed_execution(input).expect("failed to seed");
            let rx = runner.execute(input);

            // Run `after_execution` hook
            runner.after_execution(seed).await.expect("failed to run after execution hook");

            // Assert the successful result
            let result = rx.await.unwrap();
            assert_matches::assert_matches!(
                result,
                Ok(ExecOutput { done, stage_progress })
                    if done && stage_progress == previous_stage
            );

            // Validate the stage execution
            assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
        }

        // Check that unwind does not panic on no new entries within the input range.
        #[tokio::test]
        async fn unwind_no_new_entries() {
            // Set up the runner
            let mut runner = $runner::default();
            let input = crate::stage::UnwindInput::default();

            // Seed the database
            runner.seed_execution(crate::stage::ExecInput::default()).expect("failed to seed");

            // Run stage unwind
            let rx = runner.unwind(input).await;
            assert_matches::assert_matches!(
                rx,
                Ok(UnwindOutput { stage_progress }) if stage_progress == input.unwind_to
            );

            // Validate the stage unwind
            assert!(runner.validate_unwind(input).is_ok(), "unwind validation");
        }

        // Run complete execute and unwind flow.
        #[tokio::test]
        async fn unwind() {
            let (previous_stage, stage_progress) = (500, 100);

            // Set up the runner
            let mut runner = $runner::default();
            let execute_input = crate::stage::ExecInput {
                previous_stage: Some((crate::test_utils::PREV_STAGE_ID, previous_stage)),
                stage_progress: Some(stage_progress),
            };
            let seed = runner.seed_execution(execute_input).expect("failed to seed");

            // Run stage execution
            let rx = runner.execute(execute_input);
            runner.after_execution(seed).await.expect("failed to run after execution hook");

            // Assert the successful execution result
            let result = rx.await.unwrap();
            assert_matches::assert_matches!(
                result,
                Ok(ExecOutput { done, stage_progress })
                    if done && stage_progress == previous_stage
            );
            assert!(runner.validate_execution(execute_input, result.ok()).is_ok(), "execution validation");

            // Run stage unwind
            let unwind_input = crate::stage::UnwindInput {
                unwind_to: stage_progress, stage_progress: previous_stage, bad_block: None,
            };
            let rx = runner.unwind(unwind_input).await;

            // Assert the successful unwind result
            assert_matches::assert_matches!(
                rx,
                Ok(UnwindOutput { stage_progress }) if stage_progress == unwind_input.unwind_to
            );

            // Validate the stage unwind
            assert!(runner.validate_unwind(unwind_input).is_ok(), "unwind validation");
        }
    };
}

// `execute_already_reached_target` is not suitable for the headers stage thus
// included in the test suite extension
macro_rules! stage_test_suite_ext {
    ($runner:ident) => {
        crate::test_utils::stage_test_suite!($runner);

        /// Check that the execution is short-circuited if the target was already reached.
        #[tokio::test]
        async fn execute_already_reached_target() {
            let stage_progress = 1000;

            // Set up the runner
            let mut runner = $runner::default();
            let input = crate::stage::ExecInput {
                previous_stage: Some((crate::test_utils::PREV_STAGE_ID, stage_progress)),
                stage_progress: Some(stage_progress),
            };
            let seed = runner.seed_execution(input).expect("failed to seed");

            // Run stage execution
            let rx = runner.execute(input);

            // Run `after_execution` hook
            runner.after_execution(seed).await.expect("failed to run after execution hook");

            // Assert the successful result
            let result = rx.await.unwrap();
            assert_matches::assert_matches!(
                result,
                Ok(ExecOutput { done, stage_progress })
                    if done && stage_progress == stage_progress
            );

            // Validate the stage execution
            assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
        }
    };
}

pub(crate) use stage_test_suite;
pub(crate) use stage_test_suite_ext;
