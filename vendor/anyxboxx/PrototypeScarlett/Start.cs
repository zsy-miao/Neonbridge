using System;
using System.Diagnostics;
using System.IO;
using System.Windows.Forms;

internal static class Start
{
    [STAThread]
    private static void Main(string[] args)
    {
        string folder = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "PrototypeScarlett");
        string runtime = Path.Combine(folder, "runtime");
        var start = new ProcessStartInfo {
            FileName = Path.Combine(runtime, "pythonw.exe"),
            Arguments = "-I -B \"" + Path.Combine(folder, "scarlett.py") + "\"",
            WorkingDirectory = runtime,
            UseShellExecute = false,
            RedirectStandardError = true
        };
        if (args.Length == 1 && args[0] == "--demo") start.Arguments += " --demo";
        start.EnvironmentVariables["TCL_LIBRARY"] = Path.Combine(runtime, "tcl", "tcl8.6");
        start.EnvironmentVariables["TK_LIBRARY"] = Path.Combine(runtime, "tcl", "tk8.6");
        try
        {
            using (var application = Process.Start(start))
            {
                string errors = application.StandardError.ReadToEnd();
                application.WaitForExit();
                if (application.ExitCode != 0)
                    throw new InvalidOperationException("Code " + application.ExitCode + "\n" + errors);
            }
        }
        catch (Exception error)
        {
            MessageBox.Show(error.Message, "Démarrage du prototype Scarlett");
        }
    }
}
