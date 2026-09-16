package org.prs.t1.nativeui;

import android.app.Activity;
import android.os.Bundle;
import android.widget.Toast;

import java.io.IOException;

/** Home-screen entry point for the detached root native UI handoff. */
public final class LauncherActivity extends Activity {
    @Override
    public void onCreate(Bundle state) {
        super.onCreate(state);
        try {
            Process process = Runtime.getRuntime().exec(new String[] {
                    "su", "-c", "/data/local/tmp/prs-t1-launch"
            });
            process.getOutputStream().close();
            finish();
        } catch (IOException error) {
            Toast.makeText(this, "Native UI launch failed: " + error,
                    Toast.LENGTH_LONG).show();
        }
    }
}
