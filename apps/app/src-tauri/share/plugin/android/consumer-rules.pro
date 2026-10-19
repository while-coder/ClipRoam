
-keep class com.whilecode.cliproam.share.ShareNative { *; }
-keep class com.whilecode.cliproam.share.ShareUploadWorker { *; }
# These framework classes are reached through Rust JNI rather than bytecode.
-keep class androidx.work.OneTimeWorkRequest { *; }
-keep class androidx.work.OneTimeWorkRequest$Builder { *; }
-keep class androidx.work.WorkRequest$Builder { *; }
-keep class androidx.work.WorkManager { *; }
-keep class androidx.work.Data { *; }
-keep class androidx.work.Data$Builder { *; }
-keep class androidx.work.Constraints { *; }
-keep class androidx.work.Constraints$Builder { *; }
-keep class androidx.work.NetworkType { *; }
-keep class androidx.work.ExistingWorkPolicy { *; }
-keep class androidx.work.OutOfQuotaPolicy { *; }
