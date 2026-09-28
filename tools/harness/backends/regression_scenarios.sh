# Existing multi-phase and two-client scenario sequencing, sourced by regression.sh.

run_regression_scenario() {
    if [[ "$AGENT_SCENARIO" == "m94-06-save-restart-two-client-visibility" ]]; then
      run_agent_driver_phase "m94-06-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "m94-06-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "m94-06-after-restart"
        run_agent_driver_phase "m94-06-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
        if [[ "$second_client_enabled" -eq 1 ]]; then
          launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
          if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
            run_agent_driver_phase "m94-06-two-client-live-visibility" \
              --append-observations \
              --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
              --secondary-secret="$SECOND_AGENT_SECRET"
            printf 'client_agent_two_client_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
            run_agent_driver_phase "m94-06-two-client-shared-drop" \
              --append-observations \
              --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
              --secondary-secret="$SECOND_AGENT_SECRET"
            printf 'client_agent_two_client_shared_drop_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
            run_agent_driver_phase "m94-06-two-client-shared-pickup" \
              --append-observations \
              --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
              --secondary-secret="$SECOND_AGENT_SECRET"
            printf 'client_agent_two_client_shared_pickup_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
          fi
          driver_status=1
        fi
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-03-save-restart-rejoin" ]]; then
      run_agent_driver_phase "playable-03-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-03-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-03-after-restart"
        run_agent_driver_phase "playable-03-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-04-twenty-minute-survival-loop" ]]; then
      run_agent_driver_phase "playable-04-twenty-minute-survival-loop"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-04-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-04-after-restart"
        run_agent_driver_phase "playable-03-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-06-stone-tool-save-restart" ]]; then
      run_agent_driver_phase "playable-06-stone-tool-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-06-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-06-after-restart"
        run_agent_driver_phase "playable-06-stone-tool-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-13-chest-storage-save-restart" ]]; then
      run_agent_driver_phase "playable-13-chest-storage-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-13-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-13-after-restart"
        run_agent_driver_phase "playable-13-chest-storage-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-25-iron-sword-save-restart" ]]; then
      run_agent_driver_phase "playable-25-iron-sword-save-restart-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-25-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-25-after-restart"
        run_agent_driver_phase "playable-25-iron-sword-save-restart-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-29-iron-chestplate-save-restart-mitigation" ]]; then
      run_agent_driver_phase "playable-29-iron-chestplate-save-restart-mitigation-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        stop_server_gracefully "playable-29-before-restart" 1
        server_restart_count=1
        printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
        start_server "playable-29-after-restart"
        run_agent_driver_phase "playable-29-iron-chestplate-save-restart-mitigation-after" --append-observations
        driver_status="$agent_phase_status"
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-45-two-client-shared-chest-save-restart" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "playable-45-two-client-shared-chest-save-restart-before" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
          driver_status="$agent_phase_status"
          if [[ "$driver_status" -eq 0 ]]; then
            stop_server_gracefully "playable-45-before-restart" 1
            server_restart_count=1
            printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
            start_server "playable-45-after-restart"
            run_agent_driver_phase "playable-45-two-client-shared-chest-save-restart-after" \
              --append-observations \
              --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
              --secondary-secret="$SECOND_AGENT_SECRET"
            driver_status="$agent_phase_status"
          fi
        else
          driver_status=1
        fi
        printf 'client_agent_playable_two_client_shared_chest_restart_phase_exit_status=%s\n' "$driver_status" >> "$run_dir/automation-driver.txt"
      else
        printf 'error: playable-45 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-46-generated-ruin-cache" ]]; then
      run_agent_driver_phase "playable-46-generated-ruin-cache-before"
      driver_status="$agent_phase_status"
      if [[ "$driver_status" -eq 0 ]]; then
        if stop_server_gracefully "playable-46-before-restart" 1; then
          server_restart_count=1
          printf 'server_restart_count=%s\n' "$server_restart_count" >> "$run_dir/automation-driver.txt"
          start_server "playable-46-after-restart"
          run_agent_driver_phase "playable-46-generated-ruin-cache-after" --append-observations
          driver_status="$agent_phase_status"
        else
          printf 'error: playable-46 clean server stop failed; refusing after-restart phase\n' >&2
          driver_status=1
        fi
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-30-two-client-shared-log-drop-pickup" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_shared_drop_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-30 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-31-two-client-earned-shared-chest" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_shared_chest_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-31 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-32-two-client-earned-torch-block-edit" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_block_edit_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-32 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-33-two-client-player-visibility-movement" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_player_visibility_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-33 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-34-two-client-chat-message" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_chat_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-34 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-35-two-client-player-disconnect-removal" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_player_disconnect_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-35 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-36-two-client-player-reconnect-cleanup" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_player_reconnect_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-36 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-37-two-client-player-death-respawn-visibility" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_player_death_respawn_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-37 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-38-two-client-inventory-drop-handoff" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_inventory_drop_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-38 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-39-two-client-short-soak" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_short_soak_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-39 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-40-two-client-chunk-stream-crossing" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_chunk_crossing_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-40 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-41-two-client-chunk-prewarm-crossing" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_chunk_prewarm_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-41 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "playable-42-two-client-opposite-chunk-crossing" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        printf 'client_agent_playable_two_client_opposite_chunk_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        driver_status="$agent_phase_status"
      else
        printf 'error: playable-42 requires the secondary Gradle real-client adapter\n' >&2
        driver_status=1
      fi
    elif [[ "$AGENT_SCENARIO" == "m94-03b-two-client-shared-chest" || "$AGENT_SCENARIO" == "m94-03c-two-client-shared-chest-live-update" ]]; then
      if [[ "$second_client_enabled" -eq 1 ]]; then
        launch_gradle_runclient "secondary" "$SECOND_AGENT_PORT" "$SECOND_AGENT_SECRET" "$run_dir/second-client.log" second_client_pid
        if wait_for_agent_bridge "secondary" "$SECOND_AGENT_BRIDGE_URL" "$SECOND_AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
          run_agent_driver_phase "$AGENT_SCENARIO" \
            --secondary-bridge-url "$SECOND_AGENT_BRIDGE_URL" \
            --secondary-secret="$SECOND_AGENT_SECRET"
        else
          agent_phase_status=1
        fi
        if [[ "$AGENT_SCENARIO" == "m94-03c-two-client-shared-chest-live-update" ]]; then
          printf 'client_agent_two_client_shared_chest_live_update_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        else
          printf 'client_agent_two_client_shared_chest_phase_exit_status=%s\n' "$agent_phase_status" >> "$run_dir/automation-driver.txt"
        fi
        driver_status="$agent_phase_status"
      else
        run_agent_driver_phase "$AGENT_SCENARIO"
        driver_status="$agent_phase_status"
      fi
    else
      run_agent_driver_phase "$AGENT_SCENARIO"
      driver_status="$agent_phase_status"
    fi
}
